---
phase: 32-credential-foundation
plan: 01
subsystem: auth
tags: [mcp, oauth, credentials, sha2, hex, tempfile, atomic-write, backup-rotation]

# Dependency graph
requires: []
provides:
  - mcp_oauth_key: deterministic CC credential key derivation (server_name|sha256[:16] formula)
  - CredentialToken: serde struct with expiresAt camelCase for CC JSON compatibility
  - write_credential: atomic write with 5-slot backup rotation to .credentials.json
  - read_credential: reads token by server_name+url key, returns Ok(None) if absent
affects:
  - 32-02  # detect.rs reads .credentials.json to check auth status
  - 33     # OAuth callback phase writes tokens via write_credential
  - 35     # Token refresh phase reads/writes via read_credential + write_credential

# Tech tracking
tech-stack:
  added:
    - sha2 = "0.10" (workspace dep — SHA-256 for key derivation)
    - hex = "0.4" (workspace dep — hex encoding of hash bytes)
    - tempfile promoted from dev-dep to dep in crates/rightclaw (NamedTempFile::new_in for atomic writes)
  patterns:
    - Manual compact JSON string for fixed field order (not serde_json::json! which sorts alphabetically)
    - same-dir NamedTempFile::new_in + persist() = POSIX-atomic rename, avoids EXDEV
    - Ascending slot iteration for backup shift (oldest moved first, no clobber)
    - TOCTOU-safe rotation: ENOENT on rename treated as benign under concurrent access

key-files:
  created:
    - crates/rightclaw/src/mcp/mod.rs
    - crates/rightclaw/src/mcp/credentials.rs
  modified:
    - Cargo.toml (sha2 + hex in workspace.dependencies)
    - crates/rightclaw/Cargo.toml (sha2, hex, tempfile in [dependencies])
    - crates/rightclaw/src/lib.rs (pub mod mcp added)

key-decisions:
  - "serde_json::json! macro sorts keys alphabetically — build compact JSON string manually to guarantee type->url->headers field order required by CC key formula"
  - "Backup slot shift must iterate ascending (oldest first) to avoid overwriting slots before they are moved"
  - "Concurrent rotation ENOENT treated as benign — another thread already moved the slot (TOCTOU race expected under test)"

patterns-established:
  - "mcp_oauth_key format: {server_name}|hex(sha256(compact_json)[:8]) where compact_json has FIXED order type->url->headers"
  - "write_credential: read-modify-write with backup rotation before every overwrite, skip backup on first write"
  - "Atomic write: NamedTempFile::new_in(same_dir) + persist() for rename(2) atomicity"

requirements-completed:
  - CRED-01
  - CRED-02

# Metrics
duration: 5min
completed: 2026-04-03
---

# Phase 32 Plan 01: Credential Foundation Summary

**Deterministic MCP OAuth key derivation (sha256 compact JSON, type->url->headers order) and atomic .credentials.json writes with 5-slot backup rotation**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-04-03T13:11:17Z
- **Completed:** 2026-04-03T13:16:08Z
- **Tasks:** 2 (combined as TDD cycle)
- **Files modified:** 5 (+ 2 created)

## Accomplishments
- `mcp_oauth_key` passes Notion test vector: `"notion|eac663db915250e7"` confirmed
- `write_credential` atomic write with 5-slot backup rotation preserving all unrelated keys (claudeAiOauth survives)
- `read_credential` returns Ok(None) when file absent or key missing
- 12 tests pass including 10-thread concurrent write producing valid JSON

## Task Commits

Each task was committed atomically:

1. **Task 1+2: Key derivation + write/read/backup (combined TDD cycle)** - `87fd7ae` (feat)

**Plan metadata:** (docs commit below)

## Files Created/Modified
- `crates/rightclaw/src/mcp/mod.rs` — Module entry point, re-exports credentials submodule
- `crates/rightclaw/src/mcp/credentials.rs` — Full implementation: mcp_oauth_key, CredentialToken, write_credential, read_credential, rotate_backups, write_json_atomic + 12 tests
- `Cargo.toml` — Added sha2 = "0.10" and hex = "0.4" to workspace.dependencies
- `crates/rightclaw/Cargo.toml` — Promoted tempfile to [dependencies]; added sha2, hex
- `crates/rightclaw/src/lib.rs` — Added `pub mod mcp;`

## Decisions Made
- Build compact JSON manually instead of using `serde_json::json!` — the macro sorts keys alphabetically, which produces `headers->type->url` order and the wrong hash. CC expects `type->url->headers` order.
- Backup rotation slot shift must iterate ascending (old→new direction) to avoid overwriting slots before they are vacated.
- Concurrent writes: treat ENOENT on backup slot rename as benign (another thread already moved it).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] serde_json::json! sorts keys alphabetically, producing wrong credential key**
- **Found during:** Task 1 (initial test run — notion_test_vector failed)
- **Issue:** `json!` macro in serde_json sorts object keys alphabetically, producing `{"headers":{},"type":"http","url":"..."}` instead of the required `{"type":"http","url":"...","headers":{}}` field order, causing a different SHA-256 hash
- **Fix:** Replaced `json!` macro with manual `format!` string construction to guarantee exact field order matching CC's TypeScript key derivation
- **Files modified:** crates/rightclaw/src/mcp/credentials.rs
- **Verification:** `notion_test_vector` test passes with `"notion|eac663db915250e7"`
- **Committed in:** 87fd7ae

**2. [Rule 1 - Bug] Backup rotation slot shift iterated in wrong direction**
- **Found during:** Task 2 (backup_rotation_max_five_slots test failed — .bak.1 absent)
- **Issue:** Loop `for i in (1..slots.len()).rev()` processes newest slot first, overwriting older slots before they are moved (e.g., .bak renamed to .bak.1 overwrites existing .bak.1)
- **Fix:** Changed to ascending iteration `for i in 1..slots.len()` so oldest slots are moved first
- **Files modified:** crates/rightclaw/src/mcp/credentials.rs
- **Verification:** `backup_rotation_max_five_slots` test passes with .bak through .bak.4 all present
- **Committed in:** 87fd7ae

**3. [Rule 1 - Bug] Concurrent rotation fails with ENOENT when two threads try to rename the same backup slot**
- **Found during:** Task 2 (concurrent_writes_produce_valid_json — TOCTOU race in rotate_backups)
- **Issue:** Two threads both observe `slots[i].exists() == true`, then one renames the file and the other fails with `ENOENT` on its rename call
- **Fix:** Treat `ErrorKind::NotFound` on rename as benign — another thread already moved the slot
- **Files modified:** crates/rightclaw/src/mcp/credentials.rs
- **Verification:** `concurrent_writes_produce_valid_json` (10 threads) passes consistently
- **Committed in:** 87fd7ae

---

**Total deviations:** 3 auto-fixed (all Rule 1 bugs)
**Impact on plan:** All three fixes required for correctness — wrong key is invisible at runtime, wrong backup shift silently loses history, concurrent race causes panics. No scope creep.

## Issues Encountered
None beyond the three auto-fixed bugs above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- `mcp_oauth_key`, `write_credential`, `read_credential` ready for use in Phase 32-02 (MCP auth detection) and Phase 33 (OAuth callback)
- All exports from `crates/rightclaw::mcp::credentials` are public
- Notion test vector locked in as regression test — any key formula regression will be caught immediately

---
*Phase: 32-credential-foundation*
*Completed: 2026-04-03*

## Self-Check: PASSED

- FOUND: crates/rightclaw/src/mcp/credentials.rs
- FOUND: crates/rightclaw/src/mcp/mod.rs
- FOUND: .planning/phases/32-credential-foundation/32-01-SUMMARY.md
- FOUND: commit 87fd7ae (feat(32-01): mcp_oauth_key + CredentialToken + write/read_credential)
