# Phase 32: Credential Foundation - Research

**Researched:** 2026-04-03
**Domain:** MCP OAuth credential key derivation + atomic JSON file writes in Rust
**Confidence:** HIGH

## Summary

Phase 32 implements two tightly scoped primitives that all later OAuth phases depend on: (1) the deterministic key derivation formula that maps an MCP server identity to the exact key Claude Code uses in `~/.claude/.credentials.json`, and (2) a safe read-merge-write pattern for that file with atomic tmp+rename and rotating backups. Both are pure library logic — no network, no CLI commands, no process-compose interaction.

The key formula is verified against live CC data: `sha256({"type":"http","url":"<url>","headers":{}}, compact, no whitespace)[:16 hex chars]`, prepended with `serverName|`. **Field order is mandatory** — `type` → `url` → `headers`. Wrong order produces a different hash. The test vector `notion|eac663db915250e7` is independently confirmed correct (Python verification included in this document).

The write pattern follows `mcp_config.rs` for the merge logic but adds atomicity (`NamedTempFile::persist`) and backup rotation not present in the existing code. Two dependency changes are needed: `sha2` added to workspace deps (not yet present), and `tempfile` promoted from `dev-dependency` to `dependency` in the `rightclaw` crate.

**Primary recommendation:** Build `mcp/credentials.rs` as a pure, filesystem-agnostic key derivation function plus a file-writing function. Test key derivation with the Notion vector in isolation; test file writes with `tempfile::tempdir()`. Keep both concerns in the same file (under 800 LoC).

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** `type` field value for HTTP MCP servers is `"http"` — literal string
- **D-02:** `headers` field always serializes as `{}` (empty object) regardless of actual headers
- **D-03:** Full key formula: `serverName|sha256({"type":"http","url":"<url>","headers":{}}, no whitespace)[:16]`
- **D-04:** Field order in JSON input is fixed: `type`, `url`, `headers` — no whitespace, deterministic serialization
- **D-05:** Write is atomic via tmp file + POSIX rename (`tempfile` crate's `NamedTempFile` + `persist()`)
- **D-06:** Before modifying an existing `.credentials.json`, create rotating backup: `.credentials.json.bak`, `.credentials.json.bak.1`, `.credentials.json.bak.2`, `.credentials.json.bak.3`, `.credentials.json.bak.4` (5 backups, drop oldest when full)
- **D-07:** If `.credentials.json` does not exist (first write), skip backup — write fresh atomically, no error
- **D-08:** Write merges: reads existing JSON, upserts the new key, writes back — never removes unrelated keys
- **D-09:** New `mcp/` module: `crates/rightclaw/src/mcp/mod.rs` + `mcp/credentials.rs`
- **D-10:** `CredentialToken` struct with: `access_token: String`, `refresh_token: Option<String>`, `token_type: Option<String>`, `scope: Option<String>`, `expires_at: u64`
- **D-11:** `write_credential(credentials_path, server_name, server_url, token: &CredentialToken)` signature
- **D-12:** Add `sha2 = "0.10"` and `hex = "0.4"` to workspace dependencies

### Claude's Discretion
- Exact error types for `CredentialError` variants (I/O failure, JSON parse failure, backup rotation failure)
- Whether `CredentialToken` derives `Debug` with masked fields for sensitive data
- Backup rotation implementation details (rename chain vs iterate-and-shift)
- Test strategy for concurrent write safety (spawn threads vs processes)

### Deferred Ideas (OUT OF SCOPE)
- No OAuth flow (Phase 34)
- No MCP server detection (Phase 33)
- No token refresh (Phase 35)
- No process-compose restart (Phase 34+)
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CRED-01 | Operator can trust MCP OAuth tokens are written under the exact key CC expects — `serverName|sha256({"type":"...","url":"...","headers":{}}, no whitespace)[:16]` — verified by unit test against live Notion entry (`notion|eac663db915250e7`) | Key formula verified: sha2 0.10 Digest trait + hex encoding. Field order is critical (type→url→headers). serde_json compact serialization required. |
| CRED-02 | Operator can trust concurrent rightclaw invocations never corrupt `.credentials.json` — atomic write (tmp+POSIX rename) with backup before modification; never clobbers unrelated keys | NamedTempFile::persist() is the atomic primitive. Backup rotation is a sequential rename chain. Merge pattern follows mcp_config.rs. |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| sha2 | 0.10 | SHA-256 digest for key derivation | RustCrypto ecosystem standard. Implements `Digest` trait. `Sha256::digest(bytes)` returns `GenericArray`. |
| hex | 0.4 | Encode SHA-256 bytes to hex string | Already transitive dep in workspace (brought by another crate). `hex::encode(&bytes[..8])` gives 16-char hex from first 8 bytes. |
| serde_json | 1.0 | Compact JSON serialization for hash input + credentials file r/w | Already in workspace. `serde_json::to_string()` (compact, no whitespace) is the correct serialization. |
| tempfile | 3.27 | Atomic tmp+rename via `NamedTempFile::persist()` | Already in workspace deps. Currently `dev-dependency` only — must be promoted to `dependency`. |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| thiserror | 2.0 | `CredentialError` enum | Library error type — follows project convention (thiserror for modules, anyhow for binary) |
| serde | 1.0 | `CredentialToken` serialization | derive(Serialize, Deserialize) for token struct |

**Installation / Cargo.toml changes required:**

Workspace `Cargo.toml` — add to `[workspace.dependencies]`:
```toml
sha2 = "0.10"
hex = "0.4"
```

`crates/rightclaw/Cargo.toml` — promote `tempfile` and add `sha2`/`hex`:
```toml
[dependencies]
# ...existing...
sha2 = { workspace = true }
hex = { workspace = true }
tempfile = { workspace = true }  # was dev-dependency, now needed in production

[dev-dependencies]
# tempfile line removed from here (now in dependencies)
```

**Note on `hex`:** `hex = "0.4"` is already a transitive dep (0.4.3 resolved). Adding it explicitly to workspace ensures stability and correct feature set.

## Architecture Patterns

### Recommended Project Structure
```
crates/rightclaw/src/
├── mcp/
│   ├── mod.rs          # pub mod credentials; (and future detect, oauth, refresh)
│   └── credentials.rs  # mcp_oauth_key(), CredentialToken, write_credential()
├── lib.rs              # add: pub mod mcp;
└── codegen/
    └── mcp_config.rs   # existing, unchanged
```

### Pattern 1: Key Derivation (Pure Function)
**What:** Compute `serverName|sha256hex[:16]` from server name and URL. No filesystem. No state.
**When to use:** Any time you need to look up or write an MCP credential in `.credentials.json`.
**Verification:** Field order in the JSON input is locked by D-04 and is sensitive — wrong order = wrong key = invisible runtime failure.

```rust
// Source: sha2 crate + serde_json compact serialization
use sha2::{Sha256, Digest};
use serde_json::json;

pub fn mcp_oauth_key(server_name: &str, server_type: &str, url: &str) -> String {
    // Field order MUST be: type, url, headers (D-04)
    let payload = json!({
        "type": server_type,
        "url": url,
        "headers": {}
    });
    // serde_json::to_string produces compact JSON (no whitespace) — required
    let compact = serde_json::to_string(&payload).expect("serde_json serialization is infallible");
    let hash = Sha256::digest(compact.as_bytes());
    // Take first 8 bytes = 16 hex chars
    let hex_str = hex::encode(&hash[..8]);
    format!("{server_name}|{hex_str}")
}
```

**Test vector (independently verified with Python):**
```rust
assert_eq!(
    mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp"),
    "notion|eac663db915250e7"
);
```

### Pattern 2: JSON Field Order Guarantee

`serde_json::json!{}` macro preserves insertion order (uses `IndexMap` internally since serde_json 1.0.50+). Declaration order `type → url → headers` in the macro call is sufficient to guarantee correct serialization order. A `#[derive(Serialize)]` struct would also work and is equally correct — struct fields are serialized in declaration order.

**Anti-pattern:** Do NOT use `BTreeMap` for the payload — it sorts keys alphabetically (`headers < type < url`), producing a different hash.

### Pattern 3: Atomic Write with Merge (follows mcp_config.rs)

```rust
// Source: tempfile crate NamedTempFile::persist() + project mcp_config.rs pattern
use tempfile::NamedTempFile;
use std::path::Path;

fn write_credentials_atomic(
    credentials_path: &Path,
    updated: &serde_json::Value,
) -> Result<(), CredentialError> {
    let content = serde_json::to_string_pretty(updated)?;
    // NamedTempFile in same directory as target — required for POSIX rename atomicity
    let dir = credentials_path.parent().ok_or(CredentialError::InvalidPath)?;
    let mut tmp = NamedTempFile::new_in(dir)?;
    use std::io::Write;
    tmp.write_all(content.as_bytes())?;
    // persist() calls rename(2) atomically — replaces target if exists
    tmp.persist(credentials_path)?;
    Ok(())
}
```

**Critical:** `NamedTempFile::new_in(dir)` (same directory as target) is required. Cross-filesystem rename fails on some Linux configurations. `NamedTempFile::new()` (in `/tmp`) would fail if `/tmp` is on a different filesystem.

### Pattern 4: Backup Rotation

Rotate before first modification. 5 slots: `.bak`, `.bak.1`, `.bak.2`, `.bak.3`, `.bak.4`.

Rotation order (shift from oldest to newest):
1. If `.bak.4` exists → remove it (oldest dropped)
2. `.bak.3` → `.bak.4`
3. `.bak.2` → `.bak.3`
4. `.bak.1` → `.bak.2`
5. `.bak` → `.bak.1`
6. Current file → `.bak`

This is `rename(2)` for each step — fast and crash-safe at each step. If the process crashes mid-rotation, at worst one backup slot is empty; the others remain intact.

### Anti-Patterns to Avoid
- **String concatenation for JSON payload:** Fragile, easy to introduce whitespace or wrong escaping. Use `serde_json::json!{}` + `to_string()`.
- **`serde_json::to_writer` directly to file:** Not atomic. Power loss or crash mid-write corrupts the file. Always go through `NamedTempFile`.
- **`BTreeMap` for payload:** Alphabetic key ordering breaks the hash. `json!{}` macro insertion order is correct.
- **`tempfile::tempdir()` for atomic write:** `tempdir()` creates a directory, not a file. Use `NamedTempFile::new_in()`.
- **Cross-dir tmp file:** `NamedTempFile::new()` defaults to system `/tmp` which may be a different filesystem — `persist()` would fail. Always `new_in(credentials_path.parent())`.
- **`unwrap()` on backup rename:** If disk is full, rename of `.bak.3 → .bak.4` will fail. This should be a `CredentialError::BackupFailed`, not a panic.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Atomic file write | Manual write then rename | `NamedTempFile::persist()` | Handles POSIX rename, cleanup on drop if persist fails, cross-platform |
| SHA-256 | Custom hash | `sha2::Sha256::digest()` | Correctness, constant-time, no side channels |
| Hex encoding | `format!("{:02x}", byte)` loop | `hex::encode(&bytes[..8])` | Both are fine but `hex` is already a dep — no reason to hand-roll |
| JSON compact serialization | String building | `serde_json::to_string()` | Handles escaping, no whitespace guaranteed |

**Key insight:** The merge pattern from `mcp_config.rs` (read → deserialize → mutate `Value` in place → serialize → write) is correct and reusable. The only addition is wrapping the write step with `NamedTempFile`.

## Runtime State Inventory

Step 2.5: SKIPPED — This is a greenfield module addition. No rename, refactor, or migration involved. No existing runtime state to audit.

## Environment Availability

Step 2.6: SKIPPED — Phase 32 is purely code/library changes. No external tools, services, or CLIs are required beyond the existing Rust toolchain.

## Common Pitfalls

### Pitfall 1: Wrong JSON field order breaks the key
**What goes wrong:** Key derivation produces a hash that doesn't match what CC wrote. Token is written under a wrong key. CC silently ignores it. OAuth "works" but CC stays unauthenticated.
**Why it happens:** `BTreeMap`, manual struct with wrong field order, or adding whitespace to the JSON.
**How to avoid:** Always use `serde_json::json!{"type":..., "url":..., "headers":{}}` in exactly that insertion order. The Notion test vector (`notion|eac663db915250e7`) must pass before merging.
**Warning signs:** Test vector fails immediately. No runtime symptom until CC tries to call the MCP server.

### Pitfall 2: tempfile on different filesystem than target
**What goes wrong:** `NamedTempFile::persist()` returns `Err(PersistError)` with "Invalid cross-device link" (errno EXDEV).
**Why it happens:** Default `NamedTempFile::new()` creates the file in `/tmp`, which may be `tmpfs` while `~/.claude/` is on the main filesystem.
**How to avoid:** Always use `NamedTempFile::new_in(credentials_path.parent())`.
**Warning signs:** Works in dev (both on same FS), fails on systems with `/tmp` as tmpfs (common on systemd Linux).

### Pitfall 3: `tempfile` is dev-dependency — compile error in production
**What goes wrong:** `NamedTempFile` is unavailable in non-test builds because `tempfile` is listed only under `[dev-dependencies]` in `crates/rightclaw/Cargo.toml`.
**Why it happens:** Existing usage of `tempfile` is entirely in `#[cfg(test)]` modules. When `credentials.rs` uses it in production code, the crate is missing.
**How to avoid:** Move `tempfile = { workspace = true }` to `[dependencies]` (not `[dev-dependencies]`) in `crates/rightclaw/Cargo.toml`.
**Warning signs:** `cargo build` fails with "use of undeclared crate or module `tempfile`" outside test context.

### Pitfall 4: Merge reads stale/corrupt `.credentials.json`
**What goes wrong:** If `.credentials.json` was partially written (prior crash), `serde_json::from_str` fails. Error propagates and the OAuth token is never saved.
**Why it happens:** No defensive handling of parse errors on the existing file.
**How to avoid:** If parse fails, log a warning, treat the file as empty, proceed with just the new key. The backup rotation should have preserved the previous good copy. This is Claude's Discretion territory — decide in plan whether to treat parse failure as hard error or recoverable.
**Warning signs:** Integration test with a corrupted `.credentials.json` as starting state.

### Pitfall 5: Backup rotation crashes mid-chain leave inconsistent state
**What goes wrong:** If the process is killed between `.bak.2 → .bak.3` and `.bak.1 → .bak.2`, `.bak.2` is gone and `.bak.1` still has the old content.
**Why it happens:** Rotation is not atomic as a whole — only each individual rename is atomic.
**How to avoid:** This is acceptable — the primary file is untouched until the final atomic write. At worst, one backup slot is lost. This is better than losing the live file. Do not attempt to make the whole rotation atomic (not possible without journaling).
**Warning signs:** N/A — this is an accepted design tradeoff.

## Code Examples

Verified patterns from official sources and project codebase:

### CredentialToken struct
```rust
// credentials.rs
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct CredentialToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    /// Unix timestamp seconds. 0 = non-expiring (Linear pattern).
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}
```

Note: CC uses camelCase for JSON keys (`expiresAt`). Use `#[serde(rename)]` on the Rust snake_case field. Verify against live `.credentials.json` during implementation — the `discoveryState` field is documented as uncertain in STATE.md.

### Key derivation function signature
```rust
pub fn mcp_oauth_key(server_name: &str, server_type: &str, url: &str) -> String
```

### write_credential function signature (from D-11)
```rust
pub fn write_credential(
    credentials_path: &Path,
    server_name: &str,
    server_url: &str,
    token: &CredentialToken,
) -> Result<(), CredentialError>
```

### Backup rotation (rename chain pattern)
```rust
// Shift: .bak.4 dropped, then .bak.3 → .bak.4, etc.
fn rotate_backups(path: &Path) -> Result<(), CredentialError> {
    let suffixes = [".bak.4", ".bak.3", ".bak.2", ".bak.1", ".bak"];
    // Remove oldest (.bak.4)
    let oldest = path.with_extension("").with_added_extension("json.bak.4");
    // ... iterate suffixes shifting each one
}
```

(Exact implementation is Claude's Discretion — rename chain or iterate-and-shift are both correct.)

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `serde_json::Value` hash (sorted keys) | `json!{}` macro (insertion order) | serde_json 1.0.50 (2022) | Field order preserved without IndexMap boilerplate |
| Cross-dir tmp + rename | Same-dir `NamedTempFile::new_in()` | tempfile 3.x | Eliminates EXDEV errors on tmpfs systems |

## Open Questions

1. **`discoveryState` field in CredentialToken**
   - What we know: CC internal field seen in live `.credentials.json` alongside `access_token`, `expiresAt`, etc. Not in MCP spec.
   - What's unclear: Whether Phase 32 needs to preserve it during merge (likely yes — merge reads existing value and keeps it), or write it fresh (Phase 34 concern).
   - Recommendation: Use `serde_json::Value` merge (upsert by key) so unknown fields like `discoveryState` are preserved automatically. Phase 32 doesn't need to understand or define this field.

2. **`expires_at` JSON key name**
   - What we know: STATE.md says `expiresAt=0` means non-expiring. Python field analysis confirms camelCase.
   - What's unclear: Whether CC uses `expiresAt` or `expires_at` in the JSON file (Rust vs JS convention).
   - Recommendation: Inspect a live `.credentials.json` before finalizing `#[serde(rename = "expiresAt")]`. Default to camelCase (CC is a JS/TypeScript app internally).

3. **Concurrent write safety via threads vs processes**
   - What we know: The atomic rename (POSIX `rename(2)`) is the OS-level protection. The backup rotation is not atomic across the full chain.
   - What's unclear: How to write a meaningful concurrency test — spawning threads that all call `write_credential` simultaneously is easier than spawning processes, but processes better simulate concurrent `rightclaw` invocations.
   - Recommendation: Thread-based concurrency test in unit tests (simpler, sufficient to verify no `unsafe` data races). A comment noting that OS-level POSIX rename provides the true guarantee. This is Claude's Discretion.

## Sources

### Primary (HIGH confidence)
- Python independent verification of SHA-256 test vector — `notion|eac663db915250e7` confirmed correct for `{"type":"http","url":"https://mcp.notion.com/mcp","headers":{}}` compact JSON
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/mcp_config.rs` — established read-merge-write JSON pattern
- `/home/wb/dev/rightclaw/Cargo.toml` — workspace deps (tempfile 3.27, hex 0.4 transitive, serde_json 1.0)
- `/home/wb/dev/rightclaw/crates/rightclaw/Cargo.toml` — tempfile is dev-dependency only (must be promoted)
- `cargo metadata` — confirmed: sha2 not in dependency tree, hex 0.4.3 is transitive

### Secondary (MEDIUM confidence)
- serde_json `json!{}` macro insertion order behavior — documented as IndexMap-backed since 1.0.50; verified consistent with test vector
- tempfile crate `new_in()` vs `new()` cross-filesystem behavior — well-known Linux constraint, documented in tempfile crate docs

### Tertiary (LOW confidence)
- CC `.credentials.json` field names (`expiresAt` camelCase, `discoveryState`) — inferred from STATE.md notes and CC being a TypeScript app; should be verified against live file before implementation

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all deps verified in workspace or crates.io, test vector confirmed
- Key formula: HIGH — independently verified with Python SHA-256
- Architecture: HIGH — follows established project patterns from mcp_config.rs
- Pitfalls: HIGH — derived from code inspection (tempfile dev-dep issue is a real blocker)
- CredentialToken field names: LOW — inferred, not verified against live CC data

**Research date:** 2026-04-03
**Valid until:** 2026-05-03 (stable domain — sha2/tempfile APIs don't change frequently)
