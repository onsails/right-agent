---
phase: 34-core-oauth-flow
plan: 02
subsystem: mcp-oauth
tags: [oauth, discovery, dcr, pkce, token-exchange]
dependency_graph:
  requires: [34-01]
  provides: [discover_as, register_client, register_client_or_fallback, build_auth_url, exchange_token, discovery_urls]
  affects: [crates/rightclaw/src/mcp/oauth.rs, Cargo.toml]
tech_stack:
  added: [reqwest form feature]
  patterns: [RFC 9728 resource metadata discovery, RFC 8414 AS metadata, OIDC well-known, RFC 7591 DCR, S256 PKCE auth URL, form-encoded token exchange, mock TCP server for unit tests]
key_files:
  created: []
  modified:
    - crates/rightclaw/src/mcp/oauth.rs
    - Cargo.toml
    - Cargo.lock
decisions:
  - "reqwest form feature was not in workspace — added form to reqwest features in Cargo.toml (Rule 3 auto-fix)"
  - "URL encoding in build_auth_url is manual percent-encoding (no extra dep needed for basic OAuth params)"
  - "discovery_urls helper exposes URL construction logic for pure unit tests separate from HTTP integration tests"
  - "Mock TCP server approach (tokio TcpListener) used for HTTP integration tests (no mockito dep added)"
metrics:
  duration: "~4m"
  completed: "2026-04-03"
  tasks: 2
  files: 3
---

# Phase 34 Plan 02: OAuth Engine — AS Discovery, DCR, Auth URL, Token Exchange Summary

Full OAuth 2.1 engine: AS discovery (RFC 9728 -> RFC 8414 -> OIDC), Dynamic Client Registration with static clientId fallback, auth URL builder with PKCE S256, and form-encoded token exchange — all async, all tested.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | AS discovery — RFC 9728, RFC 8414, OIDC fallback chain | 15a92fa | crates/rightclaw/src/mcp/oauth.rs |
| 2 | DCR, auth URL builder, and token exchange | 15a92fa | crates/rightclaw/src/mcp/oauth.rs, Cargo.toml |

Both tasks were implemented together in a single TDD cycle: all tests written first (RED — compile errors), then all functions implemented (GREEN — 24 tests pass).

## Decisions Made

1. **reqwest form feature**: The `form` feature was absent from the workspace reqwest config. `exchange_token` requires `client.form()` for `application/x-www-form-urlencoded` POST. Added `form` to reqwest features (Rule 3 auto-fix — blocking implementation).

2. **Manual URL encoding**: `build_auth_url` uses a simple inline percent-encoder rather than pulling in `percent-encoding` or `url` crate. OAuth params (client_id, state, challenge) are base64url-safe so encoding is minimal; redirect_uri uses full percent-encoding.

3. **discovery_urls helper**: Extracted URL construction to `pub fn discovery_urls()` for pure unit tests (no HTTP). Integration tests use a minimal tokio TCP listener returning canned JSON. No `mockito` dep added.

4. **Mock HTTP via tokio TcpListener**: Per plan guidance ("test URL construction logic via helper, integration tests with TCP server"), used raw `tokio::net::TcpListener` spinning canned HTTP responses. Simple and zero extra deps.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] reqwest form feature missing**
- **Found during:** Task 2 (GREEN phase — `client.form()` call failed to compile)
- **Issue:** `form` feature not enabled on reqwest workspace dep; `RequestBuilder::form()` method unavailable
- **Fix:** Added `"form"` to reqwest features in workspace `Cargo.toml`
- **Files modified:** Cargo.toml, Cargo.lock
- **Commit:** 15a92fa

## Test Results

```
cargo test -p rightclaw --lib mcp::oauth
running 24 tests
test mcp::oauth::tests::build_auth_url_no_scope_when_none ... ok
test mcp::oauth::tests::build_auth_url_contains_required_params ... ok
test mcp::oauth::tests::discovery_urls_rfc8414_no_path ... ok
test mcp::oauth::tests::discovery_urls_rfc9728_order_is_first ... ok
test mcp::oauth::tests::discovery_urls_oidc_no_path ... ok
test mcp::oauth::tests::discovery_urls_with_path_rfc9728_appends_path ... ok
test mcp::oauth::tests::discovery_urls_all_contain_well_known ... ok
test mcp::oauth::tests::discovery_urls_without_path_rfc9728_no_trailing_slash ... ok
test mcp::oauth::tests::generate_pkce_challenge_is_43_chars ... ok
test mcp::oauth::tests::generate_pkce_challenge_matches_s256_of_verifier ... ok
test mcp::oauth::tests::generate_pkce_verifier_is_43_chars ... ok
test mcp::oauth::tests::generate_state_is_22_chars ... ok
test mcp::oauth::tests::verify_state_returns_false_for_different_lengths ... ok
test mcp::oauth::tests::verify_state_returns_true_for_matching ... ok
test mcp::oauth::tests::verify_state_returns_false_for_nonmatching ... ok
test mcp::oauth::tests::discover_as_5xx_aborts_immediately ... ok
test mcp::oauth::tests::exchange_token_non_2xx_returns_token_exchange_failed ... ok
test mcp::oauth::tests::register_client_non_2xx_returns_dcr_failed ... ok
test mcp::oauth::tests::register_client_posts_correct_body_and_returns_client_id ... ok
test mcp::oauth::tests::exchange_token_posts_form_and_returns_token_response ... ok
test mcp::oauth::tests::discover_as_rfc9728_success_returns_as_metadata ... ok
test mcp::oauth::tests::discover_as_all_404_returns_discovery_failed ... ok
test mcp::oauth::tests::register_client_or_fallback_uses_static_client_id_when_no_registration_endpoint ... ok
test mcp::oauth::tests::register_client_or_fallback_returns_missing_client_id_when_neither ... ok

test result: ok. 24 passed; 0 failed; 0 ignored
```

## Known Stubs

None. All exported functions are fully implemented.

## Self-Check: PASSED
