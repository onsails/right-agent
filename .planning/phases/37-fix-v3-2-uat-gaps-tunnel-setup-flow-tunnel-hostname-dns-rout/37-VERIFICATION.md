---
phase: 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout
verified: 2026-04-04T23:30:00Z
status: passed
score: 9/9 must-haves verified
re_verification: false
---

# Phase 37: Fix v3.2 UAT Gaps Verification Report

**Phase Goal:** Restore working tunnel setup flow — store user-supplied hostname, write DNS routing wrapper script, fix OAuth bot hostname access, add mcp handler tracing, improve UX labels and visibility.
**Verified:** 2026-04-04T23:30:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `TunnelConfig` has `hostname: String` stored field (not derived) | VERIFIED | `config.rs` line 29: `pub hostname: String` inside `TunnelConfig` struct; `hostname()` method absent |
| 2 | `tunnel_uuid()` method exists on `TunnelConfig` | VERIFIED | `config.rs` lines 35-55: `pub fn tunnel_uuid(&self) -> miette::Result<String>` — decodes single-segment and 3-segment JWT |
| 3 | `--tunnel-hostname` arg wired in `main.rs` `cmd_init` | VERIFIED | `main.rs` line 95: `tunnel_hostname: Option<String>` in `Commands::Init`; lines 186-187 destructure and pass to `cmd_init`; lines 232, 259 handle both/either/neither cases with validation |
| 4 | `AuthState::Missing` displays as "auth required" | VERIFIED | `detect.rs` line 21: `AuthState::Missing => write!(f, "auth required")` |
| 5 | `doctor.rs` has `check_tunnel_token()` function | VERIFIED | `doctor.rs` lines 665-683: `fn check_tunnel_token(tunnel_cfg: &crate::config::TunnelConfig) -> DoctorCheck`; called from `run_doctor` at lines 120-124 when tunnel is configured |
| 6 | `handler.rs` has `tracing::info!` at mcp handler entries | VERIFIED | Lines 190, 237, 292, 489, 563: all five entry points (mcp dispatch, list, auth, add, remove) log `tracing::info!` with structured `agent_dir` field |
| 7 | `cloudflared-start.sh` is written by `cmd_up` with `route dns + exec tunnel run` | VERIFIED | `main.rs` lines 558-563: `format!("#!/bin/sh\nset -e\ncloudflared tunnel route dns {uuid} {hostname}\nexec cloudflared tunnel run --token {token}\n")` written to `~/.rightclaw/scripts/cloudflared-start.sh`; chmod 0o755 on unix |
| 8 | process-compose template uses `cloudflared_script_path` | VERIFIED | `process-compose.yaml.j2` lines 28-35: `{% if cloudflared_script_path %}` block gates `cloudflared:` entry; command is `"{{ cloudflared_script_path }}"` verbatim |
| 9 | Workspace builds clean | VERIFIED | `cargo build --workspace` → `Finished dev profile` with 0 errors, 0 warnings displayed |

**Score:** 9/9 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/config.rs` | `TunnelConfig { token, hostname }` + `tunnel_uuid()` | VERIFIED | Field at line 29, method at lines 35-55 |
| `crates/rightclaw-cli/src/main.rs` | `--tunnel-hostname` CLI arg + `cloudflared-start.sh` write | VERIFIED | CLI arg at line 95, script write at lines 537-574 |
| `crates/rightclaw/src/mcp/detect.rs` | `AuthState::Missing` displays "auth required" | VERIFIED | Line 21 |
| `crates/rightclaw/src/doctor.rs` | `check_tunnel_token()` + `mcp_auth_issues()` pub | VERIFIED | Lines 665-683 and 766-782 |
| `crates/bot/src/telegram/handler.rs` | `tracing::info!` at 5 mcp handler entries | VERIFIED | Lines 190, 237, 292, 489, 563 |
| `templates/process-compose.yaml.j2` | `cloudflared_script_path`-gated cloudflared block | VERIFIED | Lines 28-35 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `cmd_up` | `cloudflared-start.sh` | `std::fs::write` + script content | WIRED | `main.rs` lines 558-563: writes script with route dns + exec tunnel run |
| `cmd_up` | `generate_process_compose` | `cloudflared_script_path` arg | WIRED | `main.rs` lines 593-602: converts path to str, passes as `script_path_str.as_deref()` |
| `generate_process_compose` | template | `cloudflared_script_path: Option<&str>` | WIRED | `process_compose.rs` line 60; template consumes at line 28-35 |
| `run_doctor` | `check_tunnel_token` | reads global config, calls fn when tunnel present | WIRED | `doctor.rs` lines 119-124 |
| `Commands::Init` | `cmd_init` | destructures `tunnel_hostname` from clap enum | WIRED | `main.rs` lines 186-187 |

### Data-Flow Trace (Level 4)

Not applicable — this phase produces CLI commands, Telegram handlers, and configuration generators, not components that render dynamic data from a remote store.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Workspace builds clean | `cargo build --workspace` | `Finished dev profile` | PASS |
| `tunnel_uuid()` method present | `grep "pub fn tunnel_uuid" crates/rightclaw/src/config.rs` | line 35 match | PASS |
| `AuthState::Missing` label | `grep '"auth required"' crates/rightclaw/src/mcp/detect.rs` | line 21 match | PASS |
| `check_tunnel_token` fn present | `grep "fn check_tunnel_token" crates/rightclaw/src/doctor.rs` | line 665 match | PASS |
| MCP handler tracing count | `grep "tracing::info!" crates/bot/src/telegram/handler.rs` | 7 lines (>= 5 required) | PASS |
| cloudflared-start.sh write | `grep "cloudflared-start.sh" crates/rightclaw-cli/src/main.rs` | 3 matches | PASS |
| template script path gate | `grep "cloudflared_script_path" templates/process-compose.yaml.j2` | 2 matches | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| D-01..D-14 (v3.2 UAT gaps) | 37-01, 37-02, 37-03 | Tunnel hostname, DNS wrapper, doctor, MCP labels, tracing | SATISFIED | All 9 specific truths verified in code |
| TUNL-01 | Phase 37 | Tunnel hostname stored and used in DNS routing wrapper | SATISFIED | `TunnelConfig.hostname` field + `cloudflared-start.sh` generation |

### Anti-Patterns Found

None found. No TODO/FIXME/placeholder comments in modified files. No stub returns. No empty handlers.

### Human Verification Required

None — all acceptance criteria are mechanically verifiable.

### Gaps Summary

No gaps. All 9 specified truths are present and wired in the actual codebase:

1. `TunnelConfig.hostname` is a stored field (not derived from JWT — Phase 36 derivation approach reversed).
2. `tunnel_uuid()` decodes both single-segment and 3-segment JWT formats.
3. `--tunnel-hostname` is wired end-to-end in `cmd_init` with validation (both/token-only/hostname-only/neither cases).
4. `AuthState::Missing` displays "auth required" (not "missing").
5. `check_tunnel_token()` calls `tunnel_uuid()` and is integrated into `run_doctor()`.
6. All five MCP handler entry points log structured `tracing::info!`.
7. `cmd_up` writes `cloudflared-start.sh` with `set -e`, `route dns`, and `exec tunnel run`.
8. process-compose template gates the cloudflared block on `cloudflared_script_path` and uses it as command.
9. `cargo build --workspace` finishes clean.

---

_Verified: 2026-04-04T23:30:00Z_
_Verifier: Claude (gsd-verifier)_
