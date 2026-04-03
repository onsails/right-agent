---
phase: 35-token-refresh
plan: "01"
subsystem: mcp-credentials
tags: [credentials, oauth, token-refresh, serde]
dependency_graph:
  requires: []
  provides: [CredentialToken.client_id, CredentialToken.client_secret]
  affects: [35-02-refresh-scheduler]
tech_stack:
  added: []
  patterns: [serde skip_serializing_if, manual Debug impl for secret redaction]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/credentials.rs
    - crates/bot/src/telegram/oauth_callback.rs
    - crates/rightclaw/src/mcp/detect.rs
key_decisions:
  - "client_id stored as Some(String) always — PendingAuth.client_id is non-optional (DCR or static fallback guarantees it)"
  - "client_secret stored as Option to handle both public and confidential clients; None = public client"
  - "skip_serializing_if on both fields ensures old credentials.json files are untouched and still valid"
metrics:
  duration: "4m"
  completed: "2026-04-03"
  tasks_completed: 2
  files_modified: 3
---

# Phase 35 Plan 01: Extend CredentialToken with client_id/client_secret Summary

**One-liner:** Extended `CredentialToken` with `client_id`/`client_secret` optional fields (serde-skipped when None) and backfilled OAuth callback to write them at flow completion, enabling refresh scheduler to POST refresh grants without additional DCR round-trips.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Extend CredentialToken with client_id and client_secret fields (TDD) | 3946e8c | credentials.rs, detect.rs |
| 2 | Backfill OAuth callback to write client_id and client_secret into credential | b4fc8cd | oauth_callback.rs |

## Decisions Made

- `client_id` is `Option<String>` with `skip_serializing_if = "Option::is_none"` — absent from JSON when None, so old credentials round-trip cleanly.
- `client_secret` uses same serde annotation and is redacted in Debug output via `as_deref().map(|_| "[REDACTED]")`.
- `PendingAuth.client_id` is always `String` (never Option) — guaranteed by DCR or static client fallback — so wrapped as `Some(pending.client_id.clone())` unconditionally.
- `detect.rs` test helper `token_with_expiry` updated with `client_id: None, client_secret: None` (Rule 1 auto-fix — struct literal exhaustiveness).

## Verification

- `cargo test -p rightclaw mcp::credentials` — 16 tests pass (12 pre-existing + 4 new)
- `cargo build --workspace` — zero errors
- `rg "client_id: Some(pending" crates/bot/src/telegram/oauth_callback.rs` — line found
- `rg "client_secret: pending" crates/bot/src/telegram/oauth_callback.rs` — line found

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed missing fields in detect.rs test helper**
- **Found during:** Task 1 GREEN phase
- **Issue:** `detect.rs::token_with_expiry` used a `CredentialToken` struct literal without the new `client_id`/`client_secret` fields — compile error
- **Fix:** Added `client_id: None, client_secret: None` to the helper
- **Files modified:** `crates/rightclaw/src/mcp/detect.rs`
- **Commit:** 3946e8c (bundled with Task 1)

## Known Stubs

None — all new fields are wired to real data from `PendingAuth`.

## Self-Check: PASSED

- `crates/rightclaw/src/mcp/credentials.rs` — exists, contains `pub client_id: Option<String>` and `pub client_secret: Option<String>`
- `crates/bot/src/telegram/oauth_callback.rs` — exists, contains `client_id: Some(pending.client_id.clone())`
- Commits `3946e8c` and `b4fc8cd` exist in git log
