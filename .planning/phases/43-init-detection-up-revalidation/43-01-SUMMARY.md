---
phase: 43-init-detection-up-revalidation
plan: "01"
subsystem: infra
tags: [chrome, mcp, config, cli, detection, rightclaw]

requires:
  - phase: 39-cloudflared-auto-tunnel
    provides: "TunnelConfig + write_global_config pattern that cmd_init refactor extends"
  - phase: 38-tunnel-refactor
    provides: "GlobalConfig struct in config.rs that we extend with ChromeConfig"

provides:
  - "ChromeConfig struct in config.rs with chrome_path + mcp_binary_path fields"
  - "GlobalConfig.chrome: Option<ChromeConfig> field with read/write support"
  - "detect_chrome_binary(), detect_mcp_binary(), brew_prefix(), detect_chrome_with_home(), detect_chrome() helpers in main.rs"
  - "--chrome-path CLI arg on rightclaw init for manual override"
  - "cmd_init() refactored to single write_global_config call at end (eliminates early-return pattern)"
  - "config.yaml always written after init regardless of tunnel/chrome detection outcome"

affects: [44-up-revalidation, chrome-mcp-injection, config-read]

tech-stack:
  added: []
  patterns:
    - "detect_X_with_home() + detect_X() testable/real split pattern (mirrors detect_cloudflared_cert_with_home)"
    - "cfg-gated detect_chrome_binary() with platform-specific candidate lists"
    - "Value-producing tunnel block: Option<TunnelConfig> = if !cert { None } else { ... Some(cfg) }"
    - "Single config write at end of cmd_init instead of early-return writes"

key-files:
  created: []
  modified:
    - crates/rightclaw/src/config.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw-cli/tests/cli_integration.rs

key-decisions:
  - "ChromeConfig added to config.rs (not main.rs) — shared type belongs in library crate"
  - "RawChromeConfig deserialization uses filter(non-empty) rather than Option<> to handle partial YAML gracefully"
  - "detect_chrome_binary() takes home: &Path even on Linux (absolute paths only) for API consistency with macOS variant"
  - "brew_prefix() compiled on all platforms but only called from macOS cfg branch — avoids dead_code on Linux"
  - "test_init_chrome_path_arg_warns_when_mcp_missing checks stdout (tracing writes to stdout by default in rightclaw)"
  - "Tunnel print moved inside else branch before Some(tunnel_config) to preserve scoping of uuid/hostname locals"

patterns-established:
  - "Single write path: cmd_init accumulates Option<T> per config section, writes once at end"
  - "Non-fatal detection: warn + None rather than error when optional dependency missing"

requirements-completed: [CHROME-01, CHROME-02, CHROME-03]

duration: 35min
completed: 2026-04-06
---

# Phase 43 Plan 01: Chrome Detection + cmd_init Single Write Path Summary

**Chrome auto-detection helpers (Linux/macOS), --chrome-path override, and cmd_init refactored to single write_global_config call with config.yaml always written**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-04-06T15:00:00Z
- **Completed:** 2026-04-06T15:35:00Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- Added `ChromeConfig` struct to `config.rs` with full YAML read/write support; `GlobalConfig` gains `chrome: Option<ChromeConfig>`
- Five detection helpers in `main.rs`: `detect_chrome_binary()` (Linux/macOS cfg-gated), `detect_mcp_binary()` (PATH + npm-global + brew), `brew_prefix()`, `detect_chrome_with_home()`, `detect_chrome()`
- `--chrome-path` override on `rightclaw init`; missing MCP binary warns and skips (non-fatal)
- `cmd_init()` tunnel block refactored from early-return + internal write to value-producing `Option<TunnelConfig>`; single `write_global_config` at end writes config.yaml unconditionally

## Task Commits

1. **Task 1: Chrome + MCP detection helpers** - `9297d83` (feat)
2. **Task 2: --chrome-path arg + cmd_init single write path** - `956389f` (feat)

## Files Created/Modified

- `crates/rightclaw/src/config.rs` — Added `ChromeConfig` struct, `GlobalConfig.chrome` field, `RawChromeConfig` deserializer, chrome serialization in `write_global_config`
- `crates/rightclaw-cli/src/main.rs` — Added 5 detection helpers, `--chrome-path` on `Init` struct, `cmd_init` signature + body refactor, 6 unit tests
- `crates/rightclaw-cli/tests/cli_integration.rs` — Added `test_init_always_writes_config` and `test_init_chrome_path_arg_warns_when_mcp_missing`

## Decisions Made

- `ChromeConfig` lives in the library crate `rightclaw` (config.rs), not main.rs — shared types belong in lib
- `RawChromeConfig` uses `filter(!empty)` rather than nested Option — avoids partially-populated chrome config from stale YAML keys
- TDD approach: wrote failing tests before implementing each task; confirmed RED then GREEN
- `detect_chrome_binary()` signature takes `home: &Path` even on Linux (where it only uses absolute paths) to keep the API uniform with the macOS variant that does use home
- Print of tunnel UUID/hostname moved inside the `else` branch (before `Some(tunnel_config)`) to keep uuid/hostname locals in scope

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical] Added ChromeConfig + GlobalConfig.chrome to config.rs**
- **Found during:** Task 1 setup
- **Issue:** Plan interfaces section showed `ChromeConfig` and `GlobalConfig.chrome` as existing, but they were absent from config.rs
- **Fix:** Added `ChromeConfig` struct, `GlobalConfig.chrome: Option<ChromeConfig>`, `RawChromeConfig` deserialization struct, chrome read/write in `read_global_config` / `write_global_config`; also fixed the existing `GlobalConfig { tunnel: Some(...) }` literal in cmd_init that was missing `chrome` field (would fail to compile once GlobalConfig gained the new field)
- **Files modified:** `crates/rightclaw/src/config.rs`, `crates/rightclaw-cli/src/main.rs` (interim `chrome: None` stub, replaced by Task 2 refactor)
- **Verification:** `cargo build --workspace` exits 0; config round-trip tests pass
- **Committed in:** `9297d83` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 2 — missing critical type definitions)
**Impact on plan:** Required to unblock compilation; no scope creep. Plan interfaces section described the intended state, not current state.

## Issues Encountered

- `test_init_always_writes_config` initially used `-y` flag which requires `--tunnel-hostname` (existing behavior). Switched to `--telegram-token` to avoid interactive prompt while keeping the test non-interactive.
- `test_init_chrome_path_arg_warns_when_mcp_missing` initially used `-y` for same reason. Fixed in same way.
- `test_status_no_running_instance` fails (pre-existing, documented in STATE.md); not introduced by this plan.

## Known Stubs

None — detection helpers are fully wired into cmd_init and produce real config output.

## Threat Flags

None — no new network endpoints, auth paths, or trust-boundary file access introduced beyond what the threat model anticipated.

## Next Phase Readiness

- `ChromeConfig` in config.yaml is ready for Phase 43-02 to read and inject into per-agent settings
- Detection helpers are tested and non-fatal — ready for use in `cmd_up` revalidation
- `write_global_config` single-write pattern established for future config sections

---
*Phase: 43-init-detection-up-revalidation*
*Completed: 2026-04-06*
