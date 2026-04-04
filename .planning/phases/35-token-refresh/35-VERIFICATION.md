---
phase: 35-token-refresh
verified: 2026-04-03T23:59:00Z
status: passed
score: 11/11 must-haves verified
re_verification: false
---

# Phase 35: Token Refresh Verification Report

**Phase Goal:** Implement token refresh scheduler — automatically renew MCP OAuth tokens before expiry
**Verified:** 2026-04-03
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | CredentialToken has client_id and client_secret optional fields that round-trip through JSON | VERIFIED | `credentials.rs` lines 36-40; 4 new tests including `old_json_round_trips_without_client_id` |
| 2 | Tokens written before Phase 35 deserialize without error (absent fields become None) | VERIFIED | `#[serde(skip_serializing_if = "Option::is_none")]` on both fields; test `old_json_round_trips_without_client_id` passes |
| 3 | client_secret is redacted in Debug output | VERIFIED | `credentials.rs` line 52: `.field("client_secret", &self.client_secret.as_deref().map(\|_\| "[REDACTED]"))`; test `debug_redacts_client_secret` passes |
| 4 | OAuth callback writes client_id and client_secret into the credential when completing the flow | VERIFIED | `oauth_callback.rs` lines 225-226: `client_id: Some(pending.client_id.clone())`, `client_secret: pending.client_secret.clone()` |
| 5 | refresh_token_for_server posts a refresh grant and writes the new token atomically | VERIFIED | `refresh.rs` lines 118-176: discovers AS, POSTs form grant, calls `write_credential` atomically |
| 6 | run_refresh_scheduler spawns one tokio task per token-bearing server | VERIFIED | `refresh.rs` lines 259-311: iterates `mcp_auth_status`, calls `tokio::spawn(run_token_refresh_loop(...))` per qualifying server |
| 7 | tokens with expires_at=0 are never scheduled (integer underflow guard) | VERIFIED | `refresh.rs` line 208: `if token.expires_at == 0 { return; }` in loop; line 300: `if credential.expires_at == 0 { continue; }` in scheduler; `deadline_from_unix(0, 600)` returns None |
| 8 | tokens without a refresh_token field are skipped with a warn log, not panicked | VERIFIED | `refresh.rs` lines 214-217: `if token.refresh_token.is_none() { warn!(...); return; }`; scheduler also checks at lines 290-296 |
| 9 | scheduler retries up to 3 times on failure, then stops scheduling for that token | VERIFIED | `refresh.rs` lines 235-244: `retries += 1; if retries >= MAX_RETRIES { error!(...); return; }` |
| 10 | Bot startup spawns the refresh scheduler as a background tokio task | VERIFIED | `bot/src/lib.rs` lines 148-162: clones credentials_path before move, spawns `rightclaw::mcp::refresh::run_refresh_scheduler` in `tokio::spawn` |
| 11 | rightclaw doctor includes mcp-tokens check with Pass/Warn based on token state | VERIFIED | `doctor.rs` line 120: `checks.push(check_mcp_tokens(home))`; 5 tests covering Pass/Warn/non-expiring cases |

**Score:** 11/11 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/mcp/credentials.rs` | Extended CredentialToken with client_id/client_secret | VERIFIED | Both fields present with skip_serializing_if, Debug impl redacts client_secret |
| `crates/bot/src/telegram/oauth_callback.rs` | Backfilled CredentialToken construction | VERIFIED | Lines 225-226 write client_id/client_secret from PendingAuth |
| `crates/rightclaw/src/mcp/refresh.rs` | Full refresh module (482 lines) | VERIFIED | Contains RefreshError, post_refresh_grant, refresh_token_for_server, run_token_refresh_loop, run_refresh_scheduler, deadline_from_unix |
| `crates/rightclaw/src/mcp/mod.rs` | pub mod refresh declaration | VERIFIED | Line 4: `pub mod refresh;` |
| `crates/bot/src/lib.rs` | tokio::spawn of run_refresh_scheduler | VERIFIED | Lines 151-162 spawn scheduler after credentials_path clone |
| `crates/rightclaw/src/doctor.rs` | check_mcp_tokens function and call in run_doctor | VERIFIED | Function at line 659; called at line 120 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `oauth_callback.rs` | `credentials.rs` | CredentialToken construction with client_id/client_secret | WIRED | Lines 225-226 confirmed |
| `refresh.rs` | `oauth.rs` | `discover_as` for token_endpoint | WIRED | Line 9 import, line 138 call |
| `refresh.rs` | `credentials.rs` | `read_credential` / `write_credential` | WIRED | Line 7 import, lines 124, 172, 195, 281 calls |
| `refresh.rs` | `detect.rs` | `mcp_auth_status` enumerates token-bearing servers | WIRED | Line 8 import, line 266 call |
| `bot/src/lib.rs` | `refresh.rs` | `tokio::spawn(run_refresh_scheduler(...))` | WIRED | Lines 155-162 confirmed |
| `doctor.rs` | `detect.rs` | `check_mcp_tokens` calls `mcp_auth_status` per agent | WIRED | Line 697 call |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `refresh.rs::refresh_token_for_server` | credential | `read_credential` → fs read | Yes — reads `~/.claude/.credentials.json` | FLOWING |
| `refresh.rs::post_refresh_grant` | TokenResponse | reqwest POST to token_endpoint | Yes — HTTP form POST with real params | FLOWING |
| `doctor.rs::check_mcp_tokens_with_creds` | statuses | `mcp_auth_status` → reads `.mcp.json` + credentials | Yes — real file I/O per agent | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| credentials tests (16) | `cargo test -p rightclaw mcp::credentials` | 16 passed | PASS |
| refresh module tests (8) | `cargo test -p rightclaw mcp::refresh` | 8 passed | PASS |
| doctor tests (5 new) | `cargo test -p rightclaw check_mcp_tokens` | 5 passed | PASS |
| workspace build | `cargo build --workspace` | Finished dev profile, 0 errors | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| REFRESH-01 | None (superseded) | On-demand CLI refresh command | SUPERSEDED | Struck through in REQUIREMENTS.md — replaced by D-01 (bot scheduler owns refresh) |
| REFRESH-02 | None (superseded) | `rightclaw up` pre-launch refresh | SUPERSEDED | Struck through in REQUIREMENTS.md — replaced by D-02 (bot startup handles refresh) |
| REFRESH-03 | 35-03-PLAN.md | `rightclaw doctor` reports missing/expired MCP tokens per agent (Warn) | SATISFIED | `check_mcp_tokens` wired into `run_doctor`; 5 tests confirm Pass/Warn/non-expiring behavior |
| REFRESH-04 | 35-01, 35-02, 35-03 PLAN.md | expires_at=0 tokens skipped by refresh loop, treated as non-expiring | SATISFIED | Guard in `deadline_from_unix` (returns None for 0), in loop (early return), in scheduler (skip), in doctor (counts as Pass) |

**Orphaned requirements check:** REQUIREMENTS.md table maps REFRESH-01 and REFRESH-02 to Phase 35 with status "Superseded". These were explicitly superseded by design decisions D-01 and D-02 recorded in REQUIREMENTS.md — not silently dropped. No plan claimed them and no implementation gap exists.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None | — | — | — | — |

No TODOs, FIXMEs, placeholder returns, empty handlers, or hardcoded empty data found in phase-modified files. All functions are fully implemented.

### Human Verification Required

None — all phase behaviors are verifiable programmatically via tests and code inspection. The scheduler runs at bot startup (not testable here without launching the full bot process), but the unit tests for `run_refresh_scheduler` confirm it correctly iterates, skips, and spawns tasks.

### Gaps Summary

No gaps. All 11 observable truths verified, all artifacts substantive and wired, all key links confirmed, all tests pass, workspace builds cleanly.

---

_Verified: 2026-04-03_
_Verifier: Claude (gsd-verifier)_
