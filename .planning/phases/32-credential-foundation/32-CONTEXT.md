# Phase 32: Credential Foundation - Context

**Gathered:** 2026-04-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Key derivation formula for MCP OAuth credential keys + atomic safe writes to `~/.claude/.credentials.json`. This is the foundation that Phases 34–35 build on — no OAuth flow, no detection, just correct keys and safe file writes.

</domain>

<decisions>
## Implementation Decisions

### SHA-256 Key Formula
- **D-01:** `type` field value for HTTP MCP servers is `"http"` — literal string, matches CC internal representation and the Notion test case
- **D-02:** `headers` field always serializes as `{}` (empty object) regardless of actual headers in `.mcp.json` — headers are not part of credential identity
- **D-03:** Full key formula: `serverName|sha256({"type":"http","url":"<url>","headers":{}}, no whitespace)[:16]`
- **D-04:** Field order in JSON input is fixed: `type`, `url`, `headers` — no whitespace, deterministic serialization

### Atomic Write + Backup
- **D-05:** Write is atomic via tmp file + POSIX rename (same dir, `tempfile` crate's `NamedTempFile` + `persist()`)
- **D-06:** Before modifying an existing `.credentials.json`, create rotating backup: `.credentials.json.bak`, `.credentials.json.bak.1`, `.credentials.json.bak.2`, `.credentials.json.bak.3`, `.credentials.json.bak.4` (5 backups, drop oldest when full)
- **D-07:** If `.credentials.json` does not exist (first write), skip backup — write fresh atomically, no error
- **D-08:** Write merges: reads existing JSON, upserts the new key, writes back — never removes unrelated keys (`claudeAiOauth`, etc.)

### Module Placement
- **D-09:** New `mcp/` module in the `rightclaw` library crate: `crates/rightclaw/src/mcp/mod.rs` + `mcp/credentials.rs`
  - Phases 33+ add `detect.rs`, `oauth.rs`, etc. to the same `mcp/` module
  - `codegen/mcp_config.rs` stays in `codegen/` — it's codegen, not MCP protocol logic

### Credential Value Schema
- **D-10:** Define `CredentialToken` struct in `credentials.rs` with all known CC fields:
  - `access_token: String` (required)
  - `refresh_token: Option<String>`
  - `token_type: Option<String>`
  - `scope: Option<String>`
  - `expires_at: u64` (0 = non-expiring, e.g. Linear)
- **D-11:** `write_credential` function signature takes `(credentials_path, server_name, server_url, token: &CredentialToken)` — caller provides all inputs, function derives the key and writes

### Dependencies
- **D-12:** Add `sha2 = "0.10"` and `hex = "0.4"` to workspace dependencies (not yet in Cargo.toml); use in `rightclaw` crate for key derivation

### Claude's Discretion
- Exact error types for `CredentialError` variants (I/O failure, JSON parse failure, backup rotation failure)
- Whether `CredentialToken` derives `Debug` with masked fields for sensitive data
- Backup rotation implementation details (rename chain vs iterate-and-shift)
- Test strategy for concurrent write safety (spawn threads vs processes)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — CRED-01 (key formula unit test), CRED-02 (atomic write with backup)

### Key Formula Evidence
- CRED-01 test vector: `mcp_oauth_key("notion", "http", "https://mcp.notion.com/mcp") == "notion|eac663db915250e7"` — this is the ground truth; implementation must pass this exact assertion

### Existing MCP Config Pattern
- `crates/rightclaw/src/codegen/mcp_config.rs` — existing non-atomic read→merge→write pattern; `credentials.rs` follows same merge logic but adds atomicity

### State (v3.2 decisions pre-loaded)
- `.planning/STATE.md` — `Accumulated Context > Decisions` section has 8 v3.2 research decisions including: `expiresAt=0` = non-expiring, `sha2 0.10` as new dep, agent must restart after OAuth write

### Workspace Cargo.toml
- `Cargo.toml` — workspace deps; `sha2` and `hex` not yet present, must be added

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `tempfile = "3.27"` already in workspace deps — use `NamedTempFile` + `persist()` for atomic writes
- `serde_json` already in workspace and rightclaw crate deps — JSON merge pattern established in `mcp_config.rs`
- Error handling pattern: `miette::miette!("...")` for user-facing errors, `thiserror` for structured types

### Established Patterns
- Read-merge-write JSON: `mcp_config.rs:generate_mcp_config()` — read existing, modify in-place, write back
- Module structure: `codegen/mod.rs` re-exports submodules — follow same pattern for `mcp/mod.rs`
- Tests in same file with `#[cfg(test)]` module; extract to `_tests.rs` if >800 LoC and tests >50%

### Integration Points
- Phase 34 will call `write_credential(credentials_path, server_name, server_url, &token)` after OAuth callback
- Phase 35 (refresh) will call `write_credential` after token refresh, and `read_credential` to check expiry
- `~/.claude/.credentials.json` path derived from `dirs::home_dir()` + `.claude/.credentials.json`

</code_context>

<specifics>
## Specific Ideas

- Rotating backups: `.bak`, `.bak.1`, `.bak.2`, `.bak.3`, `.bak.4` — 5 total, user preference
- `CredentialToken.expires_at = 0` is explicitly non-expiring (Linear pattern) — refresh loop skips these
- Key derivation is pure computation — unit test it in isolation without touching the filesystem

</specifics>

---

*Phase: 32-credential-foundation*
*Context gathered: 2026-04-03*
