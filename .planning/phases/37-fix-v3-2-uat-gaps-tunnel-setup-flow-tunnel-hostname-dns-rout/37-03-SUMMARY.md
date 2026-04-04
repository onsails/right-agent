---
phase: 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout
plan: "03"
subsystem: codegen/cloudflared-startup
tags: [cloudflared, tunnel, process-compose, dns-routing, eprintln]
dependency_graph:
  requires: [37-01]
  provides: [cloudflared-start.sh generation, DNS routing wrapper, process-compose script path]
  affects: [cmd_up, generate_process_compose, templates/process-compose.yaml.j2]
tech_stack:
  added: []
  patterns: [shell-wrapper-script, chmod-unix, eprintln-before-tui]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/codegen/process_compose.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - templates/process-compose.yaml.j2
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/doctor.rs
decisions:
  - generate_process_compose takes single cloudflared_script_path instead of tunnel_token + cloudflared_config_path
  - mcp_auth_issues() added as pub fn to doctor.rs; parses detail string from existing check_mcp_tokens
  - MCP auth warning uses eprintln! (stderr, visible before TUI); no tracing::warn!
metrics:
  duration: ~8m
  completed: "2026-04-04T23:08:25Z"
  tasks_completed: 2
  files_modified: 5
---

# Phase 37 Plan 03: DNS Routing Wrapper + Process-Compose Script Path Summary

One-liner: cloudflared-start.sh wrapper with set-e + route dns written by cmd_up; process-compose entry command is the script path; MCP warning uses eprintln before TUI.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 | Update generate_process_compose signature and template | 2322111 | process_compose.rs, process_compose_tests.rs, process-compose.yaml.j2 |
| 2 | Write cloudflared-start.sh in cmd_up + fix eprintln MCP warning | c795382 | main.rs, doctor.rs |

## What Was Built

**Task 1:** `generate_process_compose` signature simplified from `(agents, exe, debug, tunnel_token, cloudflared_config_path)` to `(agents, exe, debug, cloudflared_script_path)`. Template updated: cloudflared block now gates on `cloudflared_script_path` and uses it as the command verbatim (quoted). 4 new tests added for the cloudflared entry behavior; all 89 codegen tests green.

**Task 2:** `cmd_up` now generates `~/.rightclaw/scripts/cloudflared-start.sh` when tunnel is configured. Script content: `#!/bin/sh\nset -e\ncloudflared tunnel route dns <UUID> <HOSTNAME>\nexec cloudflared tunnel run --token <TOKEN>\n`. chmod 0o755 via `PermissionsExt` on unix. UUID extracted via `tunnel_cfg.tunnel_uuid()?`. Script path passed to `generate_process_compose`. MCP auth warning added via `rightclaw::doctor::mcp_auth_issues(home)` → `eprintln!("warn: MCP auth required: ...")` before TUI launches.

## Decisions Made

- `mcp_auth_issues()` made public in `doctor.rs` rather than duplicating check logic in main.rs — parses the existing `check_mcp_tokens` detail string.
- Shell script regenerated on every `rightclaw up` (no stale-check) — consistent with cloudflared-config.yml behavior.
- `#[cfg(unix)]` guard on chmod block — keeps code compiling on non-unix targets without dead-code warnings.

## Deviations from Plan

### Auto-added Missing Critical Functionality

**1. [Rule 2 - Missing] Added `doctor::mcp_auth_issues()` public helper**
- **Found during:** Task 2
- **Issue:** Plan referenced `tracing::warn!("MCP auth required: ...")` at line ~591 but that code did not exist in this worktree (parallel execution; MCP auth block was never written here). Acceptance criteria required `eprintln.*MCP auth required` to match.
- **Fix:** Added `pub fn mcp_auth_issues(home: &Path) -> Option<Vec<String>>` to `doctor.rs` wrapping existing `check_mcp_tokens`. Used it in `cmd_up` to emit `eprintln!` before TUI starts.
- **Files modified:** `crates/rightclaw/src/doctor.rs`, `crates/rightclaw-cli/src/main.rs`
- **Commit:** c795382

## Known Stubs

None.

## Threat Flags

None. Script written to operator-owned `~/.rightclaw/scripts/`; token embedded per T-37-07 accepted risk documented in plan threat model.

## Self-Check: PASSED

All key files exist. Both commits (2322111, c795382) verified in git log.
