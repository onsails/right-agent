---
phase: 42-chrome-config-infrastructure-mcp-injection
plan: "02"
subsystem: codegen
tags: [chrome, mcp-injection, sandbox, tdd, generate-settings, generate-mcp-config]
dependency_graph:
  requires: [42-01]
  provides: [generate_mcp_config chrome_config param, generate_settings chrome_config param, chrome-devtools MCP entry injection, allowedCommands sandbox field]
  affects:
    - crates/rightclaw/src/codegen/mcp_config.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/init.rs
tech_stack:
  added: []
  patterns: [TDD RED-GREEN, Option<&T> additive parameter, serde_json::json! insertion, Vec::push additive merge]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/mcp_config.rs
    - crates/rightclaw/src/codegen/settings.rs
    - crates/rightclaw/src/codegen/settings_tests.rs
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/init.rs
decisions:
  - generate_mcp_config callers in main.rs and init.rs receive None placeholder — Plan 03 wires the actual ChromeConfig value
  - allowed_commands emitted only when non-empty, matching the excludedCommands pattern for cleaner JSON output
  - chrome overrides placed after user SandboxOverrides block to ensure additivity (SBOX-02)
metrics:
  duration: "8m"
  completed: "2026-04-06"
  tasks: 2
  files: 5
---

# Phase 42 Plan 02: MCP + Settings Chrome Injection Summary

Both generator functions extended with `chrome_config: Option<&ChromeConfig>` parameter — `generate_mcp_config()` injects a `chrome-devtools` MCP entry with exact INJECT-02 args when Some; `generate_settings()` adds `.chrome-profile` to `allowWrite` and `chrome_path` to `allowedCommands` additively.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Extend generate_mcp_config() with chrome-devtools injection | 8dddf2b | crates/rightclaw/src/codegen/mcp_config.rs, crates/rightclaw-cli/src/main.rs, crates/rightclaw/src/init.rs |
| 2 | Extend generate_settings() with Chrome sandbox overrides | faf6c6f | crates/rightclaw/src/codegen/settings.rs, crates/rightclaw/src/codegen/settings_tests.rs |

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

- `generate_mcp_config(...)` callers in `main.rs` (line 702) and `init.rs` (line 97) pass `None` for `chrome_config`. Plan 03 wires the real `ChromeConfig` value from `read_global_config()`. This is intentional and documented in the plan — not a functional stub.
- `generate_settings(...)` caller in `main.rs` (line 619) similarly passes `None`. Plan 03 resolves this.

These stubs do not prevent Plan 02's goal (function signatures extended + tested). Plan 03 will resolve them.

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary crossings. `allowedCommands` adds operator-controlled Chrome binary path to CC sandbox allowlist; path originates from operator-written config.yaml (trusted source). Args injected as JSON array (no shell interpolation).

## Self-Check: PASSED

- [x] `crates/rightclaw/src/codegen/mcp_config.rs` — found, contains `chrome_config: Option<&ChromeConfig>` and `"chrome-devtools"` insertion
- [x] `crates/rightclaw/src/codegen/settings.rs` — found, contains `chrome_config: Option<&ChromeConfig>`, `allowed_commands` Vec, `"allowedCommands"` emission, `.chrome-profile` push
- [x] `crates/rightclaw/src/codegen/settings_tests.rs` — found, contains `chrome_config_adds_chrome_profile_to_allow_write` and `chrome_config_additive_with_user_sandbox_overrides`
- [x] Commit 8dddf2b — verified via `git log --oneline`
- [x] Commit faf6c6f — verified via `git log --oneline`
- [x] `cargo test -p rightclaw --lib codegen::mcp_config` — 16 passed, 0 failed
- [x] `cargo test -p rightclaw --lib codegen::settings` — 20 passed, 0 failed
- [x] `cargo test -p rightclaw --lib` — 352 passed, 0 failed
- [x] `cargo build --workspace` — finished dev profile, 0 errors (1 pre-existing unrelated warning)
- [x] `rg 'npx' crates/rightclaw/src/codegen/mcp_config.rs` — matches only in test strings (assert messages), not in production code
