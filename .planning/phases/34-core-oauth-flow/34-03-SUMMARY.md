---
phase: 34-core-oauth-flow
plan: "03"
subsystem: codegen/cloudflared
tags: [cloudflared, tunnel, process-compose, doctor, oauth-callback]
requires: [34-01]
provides: [cloudflared-config-generation, cloudflared-process-compose-entry, doctor-tunnel-checks]
affects: [cmd_up, doctor, process-compose-template]
tech-stack:
  added: []
  patterns: [minijinja-template, conditional-process-compose-entry, doctor-warn-checks]
key-files:
  created:
    - crates/rightclaw/src/codegen/cloudflared.rs
    - crates/rightclaw/src/codegen/cloudflared_tests.rs
    - templates/cloudflared-config.yml.j2
  modified:
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/process_compose.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - templates/process-compose.yaml.j2
    - crates/rightclaw/src/doctor.rs
    - crates/rightclaw-cli/src/main.rs
decisions:
  - cloudflared process in PC config is conditional on tunnel_token presence — bots without tunnel continue to work
  - catch-all http_status:404 rule is always present regardless of agent count — cloudflared rejects configs without it
  - doctor checks are Warn severity — tunnel is optional for non-OAuth deployments
  - socket convention is <agent_dir>/oauth-callback.sock — matches D-09 and axum plan (34-02)
metrics:
  duration: "~6 minutes"
  completed: "2026-04-03"
  tasks_completed: 2
  files_changed: 9
requirements_satisfied: [OAUTH-04, OAUTH-05, TUNL-01]
---

# Phase 34 Plan 03: cloudflared Named Tunnel Integration Summary

cloudflared ingress config generation from agent list; conditional cloudflared process-compose entry using named tunnel token; doctor checks for cloudflared binary and tunnel config presence.

## Tasks Completed

| Task | Description | Commit | Files |
|------|-------------|--------|-------|
| 1 | cloudflared config template + generate_cloudflared_config | b787727 | cloudflared.rs, cloudflared_tests.rs, cloudflared-config.yml.j2, codegen/mod.rs |
| 2 | Wire cloudflared into cmd_up + process-compose template + doctor checks | 15a92fa (merged with 34-02) | process_compose.rs, process_compose_tests.rs, process-compose.yaml.j2, doctor.rs, main.rs |

## What Was Built

### templates/cloudflared-config.yml.j2

Jinja2 template that generates a cloudflared ingress config YAML. Produces one ingress rule per agent routing `/oauth/<name>/callback` to `unix:<agent_dir>/oauth-callback.sock`. Always ends with mandatory `service: http_status:404` catch-all rule.

### crates/rightclaw/src/codegen/cloudflared.rs

`generate_cloudflared_config(agents: &[(String, PathBuf)], tunnel_url: &str) -> miette::Result<String>` — renders the template with per-agent socket paths. Follows the same minijinja pattern as `process_compose.rs`. 6 unit tests cover all acceptance criteria.

### templates/process-compose.yaml.j2 (updated)

Added `{% if tunnel_token %}` conditional block that emits a `cloudflared` process entry running `cloudflared tunnel --config <path> run --token <token>` with `on_failure` restart policy, 5s backoff, 3 max restarts.

### crates/rightclaw/src/codegen/process_compose.rs (updated)

`generate_process_compose` signature extended with `tunnel_token: Option<&str>` and `cloudflared_config_path: Option<&str>`. Both passed to the minijinja context. When `None`, the `{% if %}` block is skipped.

### crates/rightclaw-cli/src/main.rs cmd_up (updated)

Before generating process-compose config:
1. Reads `GlobalConfig` via `read_global_config`
2. If tunnel is configured: generates cloudflared ingress config and writes to `~/.rightclaw/cloudflared-config.yml`
3. Passes `tunnel_token` and config path to `generate_process_compose`

### crates/rightclaw/src/doctor.rs (updated)

Two new Warn-severity checks added to `run_doctor`:
- `check_cloudflared_binary()` — `which::which("cloudflared")`, Warn if absent, provides install URL
- `check_tunnel_config(home)` — reads GlobalConfig, Warn if `tunnel.is_none()`, provides `rightclaw init` fix command

## Test Coverage

- 6 cloudflared config generation tests (all pass): 2-agent output, hostname match, path pattern, socket service, catch-all ordering, zero-agents edge case
- All 41 existing doctor tests pass (no regressions)
- All 83 existing codegen tests pass (including updated process_compose_tests with new None params)

## Deviations from Plan

### Parallel Agent Commit Overlap

**Found during:** Task 2 commit
**Issue:** Plan 34-02 parallel agent modified the same files (process_compose.rs, main.rs, process_compose_tests.rs) before my Task 2 commit. The 34-02 agent picked up my staged changes and committed them in its own commit (15a92fa).
**Resolution:** My changes are correctly present in the codebase under commit 15a92fa. No regressions. Build and all tests pass. This is expected behavior in parallel execution mode.

None — plan executed exactly as written (modulo parallel commit overlap documented above).

## Known Stubs

None. All functions are fully implemented and tested.

## Self-Check: PASSED

- `crates/rightclaw/src/codegen/cloudflared.rs` — FOUND
- `crates/rightclaw/src/codegen/cloudflared_tests.rs` — FOUND
- `templates/cloudflared-config.yml.j2` — FOUND
- `cargo build --workspace` — exits 0
- `cargo test -p rightclaw --lib codegen::cloudflared` — 6/6 pass
- `cargo test -p rightclaw --lib doctor` — 41/41 pass
- Commit b787727 — FOUND (Task 1)
- Commit 15a92fa — FOUND (contains Task 2 changes via parallel agent)
