---
phase: 34-core-oauth-flow
verified: 2026-04-03T00:00:00Z
status: passed
score: 13/13 must-haves verified
re_verification: false
---

# Phase 34: Core OAuth Flow + Bot MCP Commands — Verification Report

**Phase Goal:** Implement the full MCP OAuth 2.1 authorization flow — OAuth engine (AS discovery, DCR, PKCE), cloudflared named tunnel integration, and Telegram bot commands (/mcp list/auth/add/remove, /doctor) — so agents can authenticate with MCP servers through the bot.
**Verified:** 2026-04-03
**Status:** PASSED
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Workspace compiles with new deps (axum, subtle, rand, base64) | ✓ VERIFIED | `cargo build --workspace` exits 0; deps present in root Cargo.toml lines 37-40 |
| 2 | verify_state uses constant-time comparison (subtle crate) | ✓ VERIFIED | `use subtle::ConstantTimeEq as _` at line 5 of oauth.rs; `ct_eq` used in verify_state |
| 3 | `rightclaw init --tunnel-token X --tunnel-url Y` writes tunnel config | ✓ VERIFIED | tunnel_token/tunnel_url args in CLI (lines 103, 106); write_global_config called at line 294 |
| 4 | PKCE generate_pkce produces correct-length base64url verifier and S256 challenge | ✓ VERIFIED | 24 oauth tests pass; verifier=43 chars, challenge=43 chars tests pass |
| 5 | AS discovery tries RFC 9728 → RFC 8414 → OIDC fallback chain | ✓ VERIFIED | All 3 well-known URLs constructed in oauth.rs (lines 132-170); 5xx-aborts, 404-fallbacks confirmed by tests |
| 6 | DCR with static clientId fallback works | ✓ VERIFIED | register_client_or_fallback function at line 345; MissingClientId variant tested |
| 7 | Token exchange sends authorization_code grant with PKCE code_verifier | ✓ VERIFIED | exchange_token at line 405; form-encoded body with code_verifier; test at line 838 |
| 8 | cloudflared config generated with per-agent ingress rules and mandatory catch-all | ✓ VERIFIED | template has `http_status:404`; 6 cloudflared tests all pass |
| 9 | cloudflared appears as process-compose entry using tunnel token from config.yaml | ✓ VERIFIED | process-compose.yaml.j2 lines 28-35: `{% if tunnel_token %}` conditional block |
| 10 | `rightclaw doctor` checks for cloudflared binary and tunnel config (Warn severity) | ✓ VERIFIED | check_cloudflared_binary() at line 606 of doctor.rs; check_tunnel_config() at line 625 |
| 11 | Each bot process embeds axum Unix socket server for OAuth callbacks | ✓ VERIFIED | run_oauth_callback_server exported from oauth_callback.rs line 300; tokio::select! at lib.rs line 176 |
| 12 | PendingAuth state cleaned up after 10 minutes; consumed one-shot on success | ✓ VERIFIED | run_pending_auth_cleanup with EXPIRY=600s at line 332; entry removed on first successful callback |
| 13 | Bot responds to /mcp list/auth/add/remove and /doctor | ✓ VERIFIED | All 5 handlers in handler.rs: handle_mcp_list, handle_mcp_auth, handle_mcp_add, handle_mcp_remove, handle_doctor |

**Score:** 13/13 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/mcp/oauth.rs` | OAuth types, PKCE, discovery, DCR, exchange | ✓ VERIFIED | Exists; all exports present; 24 tests pass |
| `crates/rightclaw/src/mcp/mod.rs` | `pub mod oauth` | ✓ VERIFIED | Line 3: `pub mod oauth;` |
| `crates/rightclaw/src/config.rs` | GlobalConfig, TunnelConfig, read/write | ✓ VERIFIED | All 4 items at lines 20, 26, 46, 67 |
| `crates/rightclaw/src/codegen/cloudflared.rs` | generate_cloudflared_config | ✓ VERIFIED | Exists; function at line 28; 6 tests pass |
| `crates/rightclaw/src/codegen/mod.rs` | `pub mod cloudflared` | ✓ VERIFIED | Line 3 |
| `templates/cloudflared-config.yml.j2` | Ingress template with catch-all | ✓ VERIFIED | Exists; `http_status:404` present |
| `templates/process-compose.yaml.j2` | cloudflared conditional process entry | ✓ VERIFIED | Lines 28-35: `{% if tunnel_token %}` block |
| `crates/bot/src/telegram/oauth_callback.rs` | axum UDS server, PendingAuthMap, cleanup | ✓ VERIFIED | run_oauth_callback_server at line 300; cleanup at line 330 |
| `crates/bot/src/telegram/handler.rs` | /mcp and /doctor command handlers | ✓ VERIFIED | All 5 handlers present |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| rightclaw-cli main.rs | config.rs | `write_global_config` in cmd_init | ✓ WIRED | Line 294 of main.rs |
| mcp/mod.rs | mcp/oauth.rs | `pub mod oauth` | ✓ WIRED | Line 3 of mod.rs |
| cmd_up in main.rs | codegen/cloudflared.rs | `generate_cloudflared_config` call | ✓ WIRED | Line 617 of main.rs; reads GlobalConfig at line 607 |
| process_compose.rs | process-compose.yaml.j2 | cloudflared entry with `{% if tunnel_token %}` | ✓ WIRED | Template lines 28-35; tunnel_token param in generate_process_compose signature |
| doctor.rs | which::which("cloudflared") | check_cloudflared_binary | ✓ WIRED | Line 606-613 of doctor.rs |
| bot lib.rs run_async | oauth_callback.rs | `tokio::select!` concurrent run | ✓ WIRED | lib.rs line 165-176: spawns axum server, awaits bind, then select! |
| handler.rs handle_mcp_auth | mcp/oauth.rs | `discover_as` call | ✓ WIRED | handler.rs line 362: `rightclaw::mcp::oauth::discover_as` |
| oauth_callback.rs | credentials.rs | `write_credential` after token exchange | ✓ WIRED | Lines 24, 229-230 of oauth_callback.rs |
| oauth_callback.rs | pc_client.rs | `restart_process` after credential write | ✓ WIRED | Line 253 of oauth_callback.rs |

---

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| handler.rs handle_mcp_auth | auth URL | discover_as → register_client_or_fallback → build_auth_url | Yes — real HTTP calls to AS endpoints | ✓ FLOWING |
| oauth_callback.rs | TokenResponse | exchange_token (HTTP POST to token_endpoint) | Yes — real HTTP POST | ✓ FLOWING |
| handler.rs handle_mcp_list | MCP server list | reads agent_dir/.mcp.json | Yes — reads actual file | ✓ FLOWING |

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| OAuth PKCE/state unit tests | `cargo test -p rightclaw --lib mcp::oauth` | 24 passed | ✓ PASS |
| Config roundtrip tests | `cargo test -p rightclaw --lib config` | 35 passed | ✓ PASS |
| cloudflared codegen tests | `cargo test -p rightclaw --lib codegen::cloudflared` | 6 passed | ✓ PASS |
| Full bot tests | `cargo test -p rightclaw-bot --lib` | 59 passed | ✓ PASS |
| Full workspace build | `cargo build --workspace` | Finished (0 errors) | ✓ PASS |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| OAUTH-01 | 34-04 | Bot-only /mcp auth entry point | ✓ SATISFIED | handle_mcp_auth in handler.rs; no CLI command |
| OAUTH-02 | 34-01, 34-02 | AS discovery RFC 9728→8414→OIDC | ✓ SATISFIED | discover_as function with 3-step fallback; tests pass |
| OAUTH-03 | 34-01, 34-02 | DCR with static clientId fallback | ✓ SATISFIED | register_client_or_fallback; MissingClientId path tested |
| OAUTH-04 | 34-03, 34-04 | cloudflared binary check before flow | ✓ SATISFIED | which::which("cloudflared") check in handle_mcp_auth line 345 |
| OAUTH-05 | 34-04 | Tunnel reachability healthcheck | ✓ SATISFIED | HTTP GET to tunnel root at handler.rs line 404-429 |
| OAUTH-06 | 34-01 | PKCE state, axum callback server | ✓ SATISFIED | PendingAuth stored in HashMap; axum server on Unix socket |
| OAUTH-07 | 34-04 | Token written to credentials.json; agent restarted | ✓ SATISFIED | write_credential at line 229; restart_process at line 253 |
| BOT-01 | 34-04 | /mcp list with auth status | ✓ SATISFIED | handle_mcp_list reads .mcp.json and credential status |
| BOT-02 | 34-04 | /mcp auth <server> OAuth flow | ✓ SATISFIED | handle_mcp_auth full flow; replies with auth URL |
| BOT-03 | 34-04 | /mcp add <config> | ✓ SATISFIED | handle_mcp_add modifies .mcp.json |
| BOT-04 | 34-04 | /mcp remove <server> | ✓ SATISFIED | handle_mcp_remove modifies .mcp.json |
| BOT-05 | 34-04 | /doctor runs rightclaw doctor | ✓ SATISFIED | handle_doctor at handler.rs line 596 |
| TUNL-01 | 34-01, 34-03 | cloudflared named tunnel init + up + doctor | ✓ SATISFIED | init writes config.yaml; up generates cloudflared-config.yml + process entry; doctor warns |

All 13 requirement IDs from ROADMAP.md Phase 34 are accounted for and satisfied.

---

### Anti-Patterns Found

No significant anti-patterns found in phase-modified files. No TODOs, FIXMEs, placeholder returns, or empty handler stubs detected.

**Note:** `test_status_no_running_instance` fails in cli integration tests — this is a pre-existing issue from Phase 3 (last touched commit `3a5d0f3`), not introduced in Phase 34.

---

### Human Verification Required

#### 1. End-to-end OAuth flow

**Test:** Configure a real cloudflared named tunnel, start rightclaw up with an agent, send `/mcp auth <server>` via Telegram, visit the returned URL, complete auth in browser, verify bot confirms success.
**Expected:** Auth URL returned in ~2s; after browser redirect, bot sends "OAuth complete — agent restarting"; agent resumes with valid token in .credentials.json.
**Why human:** Requires live cloudflared tunnel, real AS server, real Telegram bot session, real browser interaction.

#### 2. Tunnel healthcheck abort behavior

**Test:** Start bot with tunnel configured but cloudflared not running; send `/mcp auth <server>`.
**Expected:** Bot replies with "Tunnel healthcheck failed: ... Is cloudflared running?" — no partial PendingAuth state left.
**Why human:** Requires live bot session with intentionally broken tunnel.

#### 3. /mcp list auth status display

**Test:** With a mix of servers (one with valid token, one expired, one missing), send `/mcp`.
**Expected:** Per-server status rows with correct present/missing/expired labels.
**Why human:** Requires real credential file state and Telegram to verify formatting.

---

### Gaps Summary

No gaps. All must-haves verified at all four levels (exists, substantive, wired, data flowing). The workspace builds cleanly, all 94+ unit tests pass, and every requirement ID from the ROADMAP is satisfied with code evidence.

The only failing test (`test_status_no_running_instance`) is pre-existing from Phase 3 and unrelated to Phase 34 scope.

---

_Verified: 2026-04-03_
_Verifier: Claude (gsd-verifier)_
