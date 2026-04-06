---
phase: 43-init-detection-up-revalidation
plan: "02"
subsystem: cli
tags: [chrome, mcp, revalidation, cmd_up, rightclaw]

requires:
  - phase: 42-chrome-config-infrastructure-mcp-injection
    provides: "generate_settings() + generate_mcp_config() with chrome_config param; ChromeConfig struct"
  - phase: 43-init-detection-up-revalidation
    plan: "01"
    provides: "ChromeConfig added to config.rs; GlobalConfig.chrome field; detection helpers in main.rs"

provides:
  - "Per-run Chrome path revalidation in cmd_up() — both chrome_path and mcp_binary_path checked with .exists()"
  - "tracing::warn! with path display when either Chrome path missing — message contains 'no longer exists'"
  - "chrome_cfg becomes None when paths invalid — generate_settings() and generate_mcp_config() receive None"
  - "Restored generate_settings() chrome_config: Option<&ChromeConfig> param (accidentally reverted by wave 1)"
  - "Restored generate_mcp_config() rightclaw_home + chrome_config params + chrome-devtools injection (reverted)"
  - "Restored RC_RIGHTCLAW_HOME env var in rightmemory MCP entry (reverted)"
  - "Restored 12 chrome-specific unit tests in settings_tests.rs + mcp_config.rs (reverted)"
  - "Integration test: test_up_warns_when_chrome_path_missing"

affects: [chrome-injection, mcp-config, settings-generation]

tech-stack:
  added: []
  patterns:
    - "Revalidation-before-health-check: chrome path check hoisted to top of cmd_up() so warn fires even when already-running blocks"
    - "match guard pattern: Some(cfg) if !cfg.path.exists() => warn + None"
    - "Non-fatal validation: chrome_cfg becomes None; agents start regardless"

key-files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/tests/cli_integration.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw/src/codegen/mcp_config.rs
    - crates/rightclaw/src/config.rs
    - crates/rightclaw/src/init.rs

key-decisions:
  - "Revalidation hoisted before health check — warn fires even when process-compose already running (affects test design)"
  - "tracing::warn! goes to stdout (tracing_subscriber default) — integration test checks stdout not stderr"
  - "Wave 1 agent regressions fixed as Rule 1 (bug) — generate_settings/generate_mcp_config chrome params restored"

patterns-established:
  - "Validate config early in cmd_up (before TCP health check) so user sees config issues regardless of runtime state"

requirements-completed: [INJECT-03]

duration: 45min
completed: 2026-04-06
---

# Phase 43 Plan 02: Per-run Chrome Path Revalidation Summary

**Per-run revalidation of Chrome paths in cmd_up() — missing paths warn and skip injection, agents always start**

## Performance

- **Duration:** ~45 min
- **Completed:** 2026-04-06
- **Tasks:** 1 (TDD)
- **Files modified:** 7

## Accomplishments

- Added revalidation match block in `cmd_up()`: checks `chrome_path.exists()` and `mcp_binary_path.exists()` on every `up` invocation
- Missing path emits `tracing::warn!` with the exact path and "no longer exists" message
- Effective `chrome_cfg` becomes `None` when either path is missing — `generate_settings()` and `generate_mcp_config()` receive `None`, no injection occurs
- Agents start normally regardless of Chrome validation result (non-fatal)
- Revalidation hoisted before the TCP health check so warnings appear even when process-compose is already running
- Integration test `test_up_warns_when_chrome_path_missing` verifies warn message in stdout

## Task Commits

1. **Task 1: Per-run Chrome path revalidation + restore phase 42 regressions** - `5f577c3` (feat)

## Files Created/Modified

- `crates/rightclaw-cli/src/main.rs` — global_cfg hoisted to top of cmd_up(), revalidation match block added, generate_settings + generate_mcp_config call sites updated
- `crates/rightclaw-cli/tests/cli_integration.rs` — added `test_up_warns_when_chrome_path_missing`
- `crates/rightclaw/src/codegen/settings.rs` — restored `chrome_config: Option<&ChromeConfig>` param, `allowed_commands` vec, chrome block
- `crates/rightclaw/src/codegen/settings_tests.rs` — restored `use ChromeConfig` import + 6 chrome-specific tests; fixed all existing call sites to pass new `None` arg
- `crates/rightclaw/src/codegen/mcp_config.rs` — restored `rightclaw_home` + `chrome_config` params, chrome-devtools injection, `RC_RIGHTCLAW_HOME` env; fixed all test call sites; added 6 new chrome tests
- `crates/rightclaw/src/config.rs` — fixed 2 test `GlobalConfig` literals missing `chrome: None` field
- `crates/rightclaw/src/init.rs` — fixed `generate_settings()` call to pass new `None` arg

## Deviations from Plan

### Auto-fixed Issues (Rule 1 — Bug)

**1. [Rule 1 - Bug] Restored Phase 42 chrome injection code reverted by wave 1 agent**
- **Found during:** Task 1 implementation setup
- **Issue:** The wave 1 (43-01) agent accidentally reverted Phase 42 chrome injection when it rewrote `config.rs` and `main.rs`. Specifically: `generate_settings()` lost its `chrome_config` param, `generate_mcp_config()` lost `rightclaw_home` + `chrome_config` params and chrome-devtools injection, `main.rs cmd_up()` lost `chrome_cfg` extraction and passing, `settings_tests.rs` lost 6 chrome tests, `mcp_config.rs` lost 7 chrome tests including `RC_RIGHTCLAW_HOME` env test
- **Fix:** Restored all reverted code by diffing against commit `faf6c6f` (phase 42-02) and `5075482` (phase 42-03). Also fixed all downstream call sites (`init.rs`, `config.rs` tests, `settings_tests.rs` existing calls)
- **Files modified:** All 7 files listed above
- **Verification:** `cargo test --workspace` — 343 lib tests pass, 23 CLI integration tests pass, 1 pre-existing failure (`test_status_no_running_instance`)
- **Commit:** `5f577c3`

**2. [Rule 1 - Bug] Hoisted revalidation before health check for testability**
- **Found during:** Test RED phase — test failed with "already running" before chrome warn could fire
- **Issue:** Original plan placed revalidation after agent discovery (mid-function). When process-compose is running in test env, the health check fires first, preventing the warn from appearing in test output
- **Fix:** Moved `global_cfg` read + revalidation block to the top of `cmd_up()`, immediately after `verify_dependencies()` and before the health check
- **Files modified:** `crates/rightclaw-cli/src/main.rs`
- **Commit:** `5f577c3` (same commit)

**3. [Rule 1 - Bug] Test checks stdout not stderr**
- **Found during:** Test GREEN phase — test checked stderr but tracing_subscriber writes to stdout by default
- **Fix:** Updated test assertion to check `stdout` (consistent with `test_init_chrome_path_arg_warns_when_mcp_missing` which also uses stdout)
- **Files modified:** `crates/rightclaw-cli/tests/cli_integration.rs`
- **Commit:** `5f577c3` (same commit)

---

**Total deviations:** 3 auto-fixed (Rule 1 — bugs from wave 1 regression + test environment adaptation)
**Impact on plan:** Required to restore phase 42 functionality before revalidation could be added. Plan assumed `generate_settings`/`generate_mcp_config` already had chrome params — they were absent due to wave 1 agent revert.

## Build and Test Results

- `cargo build --workspace` — exit 0. 1 pre-existing warning (`brew_prefix` dead_code from 43-01, cfg-gated)
- `cargo test -p rightclaw --lib` — 343 passed, 0 failed (up from 331: +12 chrome tests restored)
- `cargo test -p rightclaw-cli` — 23 passed, 1 failed (`test_status_no_running_instance` — pre-existing)

## Known Stubs

None — all chrome config paths are fully wired end-to-end.

## Threat Flags

None — no new network endpoints or trust boundaries introduced. Path values logged in `tracing::warn!` were written by the operator at init time (T-43-05 accepted in plan threat model).

## Self-Check: PASSED

- `crates/rightclaw-cli/src/main.rs` — FOUND
- `crates/rightclaw/src/codegen/settings.rs` — FOUND
- `crates/rightclaw/src/codegen/mcp_config.rs` — FOUND
- `.planning/phases/43-init-detection-up-revalidation/43-02-SUMMARY.md` — FOUND
- commit `5f577c3` — FOUND in git log
