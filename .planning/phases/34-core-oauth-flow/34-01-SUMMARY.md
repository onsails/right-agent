---
phase: 34-core-oauth-flow
plan: 01
subsystem: mcp-oauth
tags: [oauth, pkce, config, tunnel, dependencies]
dependency_graph:
  requires: [33-01]
  provides: [mcp::oauth types, GlobalConfig, write_global_config, init --tunnel-token/--tunnel-url]
  affects: [crates/rightclaw/src/mcp/oauth.rs, crates/rightclaw/src/config.rs, crates/rightclaw-cli/src/main.rs]
tech_stack:
  added: [axum=0.8, subtle=2.6, rand=0.10, base64=0.22]
  patterns: [PKCE S256 via SHA-256 + base64url-no-pad, constant-time state comparison via subtle::ConstantTimeEq, manual YAML write (serde-saphyr is deserialize-only)]
key_files:
  created:
    - crates/rightclaw/src/mcp/oauth.rs
  modified:
    - crates/rightclaw/src/mcp/mod.rs
    - crates/rightclaw/src/config.rs
    - crates/rightclaw-cli/src/main.rs
    - Cargo.toml
    - crates/rightclaw/Cargo.toml
    - crates/bot/Cargo.toml
    - .planning/REQUIREMENTS.md
    - .planning/ROADMAP.md
decisions:
  - "rand 0.10 uses rand::RngExt trait (not Rng or RngCore) for fill() method on ThreadRng"
  - "serde-saphyr is deserialize-only — GlobalConfig YAML write uses manual string formatting"
  - "TunnelConfig in GlobalConfig uses plain fields (not serde derives) since write is manual"
metrics:
  duration: "7m 35s"
  completed: "2026-04-03"
  tasks: 3
  files: 8
---

# Phase 34 Plan 01: Foundation — OAuth Types, GlobalConfig, Tunnel Init Summary

OAuth type definitions + PKCE/state utilities in mcp::oauth, GlobalConfig with TunnelConfig read/write from config.yaml, and `rightclaw init --tunnel-token/--tunnel-url` CLI extension. All 13 Phase 34 requirements mapped in planning docs per D-10.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 0 | Update REQUIREMENTS.md and ROADMAP.md per D-10 | e4278d6 | .planning/REQUIREMENTS.md, .planning/ROADMAP.md |
| 1 | Add workspace deps + OAuth types module with PKCE/state utilities | 7f25a4f | Cargo.toml, crates/rightclaw/Cargo.toml, crates/bot/Cargo.toml, crates/rightclaw/src/mcp/oauth.rs, crates/rightclaw/src/mcp/mod.rs |
| 2 | GlobalConfig struct + read/write + rightclaw init --tunnel-token --tunnel-url | 173bba2 | crates/rightclaw/src/config.rs, crates/rightclaw-cli/src/main.rs |

## Decisions Made

1. **rand 0.10 API change**: rand 0.10 uses `rand::RngExt` trait for the `fill()` method on `ThreadRng`. Neither `rand::Rng` nor `rand::RngCore` provide `fill()` directly — the compiler error pointed at `rand::RngExt`. RESEARCH.md noted `rand::rng()` (new API) but not the trait import requirement. Fixed by importing `rand::RngExt as _`.

2. **Manual YAML write for GlobalConfig**: serde-saphyr is deserialize-only (as noted in RESEARCH.md). GlobalConfig schema is tiny (2 fields) so manual string formatting is chosen over an extra serde_yaml/toml dep. Quote-escaping applied defensively for token and URL values.

3. **RawGlobalConfig helper structs**: Added `RawGlobalConfig` and `RawTunnelConfig` structs with `#[derive(Deserialize)]` as the deserialization target for serde-saphyr, then map to the public `GlobalConfig`/`TunnelConfig` types. This keeps the public types clean (no serde derives needed on them).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] rand 0.10 RngExt trait import required**
- **Found during:** Task 1 (GREEN phase)
- **Issue:** Plan specified `rand::rng().fill_bytes(&mut bytes)` but rand 0.10 reorganized traits — `fill_bytes` is on `RngCore` which requires different import; `fill()` on `RngExt` is the idiomatic rand 0.10 API
- **Fix:** Changed import to `use rand::RngExt as _` and method to `.fill(&mut bytes)`
- **Files modified:** crates/rightclaw/src/mcp/oauth.rs
- **Commit:** 7f25a4f

## Test Results

```
cargo test -p rightclaw --lib mcp::oauth
running 7 tests
test mcp::oauth::tests::generate_pkce_challenge_is_43_chars ... ok
test mcp::oauth::tests::generate_state_is_22_chars ... ok
test mcp::oauth::tests::generate_pkce_verifier_is_43_chars ... ok
test mcp::oauth::tests::generate_pkce_challenge_matches_s256_of_verifier ... ok
test mcp::oauth::tests::verify_state_returns_false_for_different_lengths ... ok
test mcp::oauth::tests::verify_state_returns_true_for_matching ... ok
test mcp::oauth::tests::verify_state_returns_false_for_nonmatching ... ok
test result: ok. 7 passed; 0 failed; 0 ignored

cargo test -p rightclaw --lib config (config::tests subset)
test config::tests::read_global_config_returns_default_when_file_missing ... ok
test config::tests::write_then_read_global_config_roundtrips_tunnel ... ok
test config::tests::write_global_config_creates_valid_yaml ... ok
test config::tests::read_global_config_parses_yaml_with_tunnel_fields ... ok
test result: ok. 35 passed; 0 failed
```

## Known Stubs

None. All types and functions are fully implemented.

## Self-Check: PASSED
