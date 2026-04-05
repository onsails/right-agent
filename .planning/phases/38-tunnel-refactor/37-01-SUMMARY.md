---
phase: 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout
plan: "01"
subsystem: config
tags: [tunnel, config, cli, init]
dependency_graph:
  requires: []
  provides: [TunnelConfig.hostname, tunnel_uuid(), cmd_init --tunnel-hostname]
  affects: [crates/rightclaw/src/config.rs, crates/rightclaw-cli/src/main.rs]
tech_stack:
  added: [base64 = "0.22"]
  patterns: [stored-field over derived, fail-fast validation, TDD]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/config.rs
    - crates/rightclaw-cli/src/main.rs
    - Cargo.toml
    - crates/rightclaw/Cargo.toml
decisions:
  - "Store hostname as explicit field in TunnelConfig (reversed Phase 36 JWT-derivation approach)"
  - "tunnel_uuid() returns raw UUID string — no .cfargotunnel.com suffix"
  - "Validate bare domain hostname before write — reject https:// and http:// prefix"
  - "Both --tunnel-token and --tunnel-hostname must be provided together or not at all"
metrics:
  duration: ~12min
  completed: "2026-04-04T22:35:55Z"
  tasks_completed: 2
  files_modified: 4
---

# Phase 37 Plan 01: TunnelConfig hostname field + --tunnel-hostname CLI arg — Summary

TunnelConfig gains stored `hostname: String` field and `tunnel_uuid()` (raw UUID extraction); `hostname()` derivation method removed; `--tunnel-hostname` wired into `rightclaw init` with validation.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add hostname field to TunnelConfig + tunnel_uuid() | 3939e1f | config.rs, Cargo.toml (x2) |
| 2 | Wire --tunnel-hostname into cmd_init with validation | 4e27d2c | main.rs |

## What Was Built

**Task 1 — TunnelConfig struct overhaul (`config.rs`):**
- `TunnelConfig { token: String, hostname: String }` — both fields required
- `tunnel_uuid()` method: decodes single-segment (real CF token) or 3-segment JWT, returns raw UUID (e.g. `7a2155a5-2ab3-4fcd-9a45-e61cce474879`) — no `.cfargotunnel.com` suffix
- `hostname()` method removed entirely
- `RawTunnelConfig` deserialization struct gains `hostname: String`
- `read_global_config` / `write_global_config` both handle `hostname` field
- 9 new tests covering: valid JWT decode, single-segment CF token, wrong segment count, invalid base64, missing `t` field, write+read roundtrip, hostname in written YAML, YAML parse
- `base64 = "0.22"` added to workspace and rightclaw crate

**Task 2 — `cmd_init` CLI wiring (`main.rs`):**
- `Commands::Init` gains `tunnel_token: Option<String>` and `tunnel_hostname: Option<String>` args
- Dispatch match arm destructures and passes both to `cmd_init`
- `cmd_init` signature extended with `tunnel_token: Option<&str>`, `tunnel_hostname: Option<&str>`
- Validation logic (match on both):
  - Both present: validate bare domain (reject `https://`/`http://` prefix), write `TunnelConfig`, print UUID confirmation
  - Token only: fail with `--tunnel-hostname is required when --tunnel-token is provided`
  - Hostname only: fail with `--tunnel-token is required when --tunnel-hostname is provided`
  - Neither: skip (no tunnel config written)

## Verification

```
cargo test -p rightclaw --lib config   → 40 passed, 0 failed
cargo build --workspace                → clean, no errors
grep "pub fn hostname" config.rs       → (empty — method absent)
grep "pub hostname: String" config.rs  → match at line 29
```

## Deviations from Plan

None — plan executed exactly as written.

The worktree started from an older base (before Phase 36 TunnelConfig was added), so Task 1 built the struct from scratch rather than modifying an existing one. End result is identical to the plan spec.

## Known Stubs

None.

## Threat Flags

None — T-37-01 (hostname scheme validation) mitigated as planned. T-37-02 and T-37-03 accepted per threat register.

## Self-Check: PASSED
