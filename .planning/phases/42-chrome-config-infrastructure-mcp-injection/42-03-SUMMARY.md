---
phase: 42-chrome-config-infrastructure-mcp-injection
plan: "03"
subsystem: cli
tags: [chrome, cmd_up, wiring, global_cfg, chrome_cfg]
dependency_graph:
  requires: [42-01, 42-02]
  provides: [cmd_up chrome_cfg wiring, end-to-end chrome injection on rightclaw up]
  affects:
    - crates/rightclaw-cli/src/main.rs
tech_stack:
  added: []
  patterns: [hoist before loop, Option<&T> pass-through, single global_cfg read]
key_files:
  created: []
  modified:
    - crates/rightclaw-cli/src/main.rs
decisions:
  - global_cfg hoisted before per-agent loop so both chrome_cfg and tunnel block share the same binding (single read, no borrow conflict)
  - chrome_cfg is Option<&ChromeConfig> extracted once — zero-cost when Chrome absent (None path, generators no-op)
  - Tunnel block after the loop references the hoisted global_cfg binding unchanged — no additional edits needed
metrics:
  duration: "3m"
  completed: "2026-04-06"
  tasks: 1
  files: 1
---

# Phase 42 Plan 03: cmd_up() Chrome Wiring Summary

cmd_up() now reads global config once before the per-agent loop, extracts chrome_cfg, and passes it to both generate_settings() and generate_mcp_config() — completing the end-to-end chrome injection flow for INJECT-01, INJECT-02, SBOX-01, SBOX-02.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Hoist global_cfg read and wire chrome_cfg into cmd_up() | 5075482 | crates/rightclaw-cli/src/main.rs |

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — chrome_cfg is fully wired. When `config.yaml` has no `chrome:` section, chrome_cfg is None and both generators silently skip chrome injection (D-14 behavior).

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary changes. global_cfg read moved earlier in cmd_up() scope; all existing error propagation (?) unchanged.

## Self-Check: PASSED

- [x] `crates/rightclaw-cli/src/main.rs` — found, contains `let chrome_cfg = global_cfg.chrome.as_ref()`
- [x] `rg "chrome_cfg" crates/rightclaw-cli/src/main.rs` — 3 matches (let binding + 2 call sites)
- [x] `rg "let global_cfg = rightclaw::config::read_global_config" crates/rightclaw-cli/src/main.rs` — exactly 1 match
- [x] `generate_settings(..., chrome_cfg)` — verified at line 624
- [x] `generate_mcp_config(..., chrome_cfg)` — verified at line 707
- [x] Commit 5075482 — verified via `git log --oneline -1`
- [x] `cargo build --workspace` — finished dev profile, 0 errors (1 pre-existing unrelated warning)
- [x] `cargo test --workspace` — 20 passed, 1 pre-existing failure (test_status_no_running_instance, documented in PROJECT.md)
