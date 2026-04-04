# Phase 36: auto-derive-cfargotunnel-hostname-from-tunnel-token-jwt - Context

**Gathered:** 2026-04-04
**Status:** Ready for planning

<domain>
## Phase Boundary

Remove `--tunnel-hostname` from the CLI entirely. Auto-derive the public hostname from the cloudflared named tunnel token JWT: decode JWT payload (base64url + serde_json), extract `t` field (tunnel UUID), construct `<uuid>.cfargotunnel.com`. No external connectivity required — derivation is pure local computation.

Scope: `TunnelConfig` struct, `config.yaml` schema, `rightclaw init` CLI arg removal, `cmd_init` validation, cloudflared config generation path.

</domain>

<decisions>
## Implementation Decisions

### --tunnel-hostname removal
- **D-01:** Remove `--tunnel-hostname` CLI arg completely from `rightclaw init`. Hard remove, no deprecation path.
- **D-02:** Existing `config.yaml` files with `hostname` field — ignore silently. `RawTunnelConfig` serde struct drops `hostname` field; unknown YAML fields are silently ignored by serde-saphyr default behavior. No migration, no error.
- **D-03:** `cmd_init` current guard `"both --tunnel-token and --tunnel-hostname are required together"` — removed entirely. Only `--tunnel-token` required for tunnel setup.

### TunnelConfig struct
- **D-04:** Remove `hostname: String` from `TunnelConfig`. Struct becomes `{ token: String }` only.
- **D-05:** Add `impl TunnelConfig { pub fn hostname(&self) -> miette::Result<String> }` — derives hostname on every call. No caching. Callers call `.hostname()?` when they need it.
- **D-06:** `write_global_config` writes only `token` field under `tunnel:` key. YAML shrinks from 3 lines to 2.

### JWT decode algorithm
- **D-07:** No new deps. Use `base64 = "0.22"` (already in workspace) + `serde_json` (already in workspace).
- **D-08:** Token format: cloudflared named tunnel token is a JWT with 3 dot-separated segments (`header.payload.signature`). Split by `.`, take segment index 1 (payload), base64url-decode with `base64::engine::general_purpose::URL_SAFE_NO_PAD`, parse JSON, extract string field `"t"` (tunnel UUID).
- **D-09:** Derived hostname: `format!("{}.cfargotunnel.com", uuid)` — always the default cfargotunnel.com subdomain.
- **D-10:** Error messages: "tunnel token has wrong number of segments (expected 3, got N)" and "tunnel token payload missing 't' field" — clear, actionable.

### Validation placement
- **D-11:** `cmd_init` calls `tunnel_config.hostname()?` immediately after constructing the config — fails fast before writing `config.yaml`. Prints derived hostname to stdout: `Tunnel hostname: <uuid>.cfargotunnel.com`.
- **D-12:** `cmd_up` (cloudflared config generation path) calls `tunnel_cfg.hostname()?` again when building the cloudflared ingress config. Defensive re-validation — catches corrupted config.yaml tokens.

### Claude's Discretion
- Exact error type/context wording beyond D-10
- Whether `hostname()` lives on `TunnelConfig` or as a free function in `config.rs`
- Test structure (unit tests for decode logic + integration test for init flow)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Core files to modify
- `crates/rightclaw/src/config.rs` — `TunnelConfig` struct, `RawTunnelConfig`, `read_global_config`, `write_global_config`, all tests
- `crates/rightclaw-cli/src/main.rs` — `Commands::Init` args, `cmd_init` validation, `cmd_up` cloudflared path (lines ~103, ~202, ~249-309, ~625-660)
- `crates/rightclaw/src/codegen/cloudflared.rs` — `generate_cloudflared_config` takes `tunnel_hostname: &str`, callers pass derived hostname
- `crates/rightclaw/src/codegen/cloudflared_tests.rs` — tests using hardcoded hostname strings

### No external specs
Requirements fully captured in decisions above.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `base64::engine::general_purpose::URL_SAFE_NO_PAD` — correct engine for JWT payload decoding (no padding)
- `serde_json::Value` — sufficient for extracting single `"t"` field without defining a struct

### Established Patterns
- `TunnelConfig` is constructed in `cmd_init` and stored via `write_global_config` — same pattern continues
- Error propagation via `miette::miette!("...")` with `?` — consistent with existing config.rs style
- `generate_cloudflared_config` already takes `tunnel_hostname: &str` — call site becomes `tunnel_cfg.hostname()?`

### Integration Points
- `cmd_up` at line ~636: `generate_cloudflared_config(&agent_pairs, &tunnel_cfg.hostname)` → `generate_cloudflared_config(&agent_pairs, &tunnel_cfg.hostname()?)`
- `cmd_up` at line ~625-660: `tunnel.hostname` field access → method call `tunnel.hostname()?`
- All tests in `config.rs` that construct `TunnelConfig { token: ..., hostname: ... }` need hostname field removed

</code_context>

<specifics>
## Specific Ideas

- "пока что" (for now) — user explicitly requested hard remove of `--tunnel-hostname`, not deprecation. Future phases may add `--tunnel-hostname` back as an override for custom domains, but that is not in scope here.

</specifics>

<deferred>
## Deferred Ideas

- Custom domain override via `--tunnel-hostname` — possible future phase if user configures a custom Cloudflare subdomain instead of `*.cfargotunnel.com`

</deferred>

---

*Phase: 36-auto-derive-cfargotunnel-hostname-from-tunnel-token-jwt*
*Context gathered: 2026-04-04*
