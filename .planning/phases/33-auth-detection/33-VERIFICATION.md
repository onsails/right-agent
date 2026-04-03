---
phase: 33-auth-detection
verified: 2026-04-03T15:30:00Z
status: passed
score: 5/5 must-haves verified
re_verification: false
---

# Phase 33: Auth Detection Verification Report

**Phase Goal:** Operators can see which MCP servers need OAuth and get warned before launching agents with unauthenticated servers
**Verified:** 2026-04-03T15:30:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `rightclaw mcp status` prints servers grouped by agent with state present/missing/expired | ✓ VERIFIED | `cmd_mcp_status` in main.rs:323 prints `agent.name:` header + `s.name s.state` per server; `./target/debug/rightclaw mcp status --help` confirms subcommand exists |
| 2 | `rightclaw mcp status --agent <name>` filters to single agent; unknown name returns error | ✓ VERIFIED | main.rs:328-333 filters by name, returns `Err(miette::miette!("agent '{name}' not found"))` on empty match |
| 3 | `rightclaw up` emits a single `tracing::warn!` line naming each agent+server pair with missing or expired state; no warn when all tokens are present | ✓ VERIFIED | main.rs:547-569: `auth_issues` Vec collects `agent/server (state)` strings, emits single `tracing::warn!("MCP auth required: {}", auth_issues.join(", "))` only when non-empty |
| 4 | Servers without `url` field in .mcp.json are silently skipped (stdio servers) | ✓ VERIFIED | detect.rs:64-67: `None => continue, // stdio server — skip silently`; Test 7 (`stdio_server_without_url_is_skipped`) passes |
| 5 | Token with `expires_at == 0` reports as Present, not Expired | ✓ VERIFIED | detect.rs:74: `if token.expires_at > 0 && token.expires_at < now_unix`; Test 1 (`expires_at_zero_is_present`) passes |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/mcp/detect.rs` | AuthState enum, ServerStatus struct, mcp_auth_status function | ✓ VERIFIED | All three exported; 253 lines; 9 tests |
| `crates/rightclaw/src/mcp/mod.rs` | `pub mod detect` declaration | ✓ VERIFIED | Line 2: `pub mod detect;` |
| `crates/rightclaw-cli/src/main.rs` | McpCommands enum, Commands::Mcp variant, cmd_mcp_status function, cmd_up auth warn block | ✓ VERIFIED | McpCommands at line 81; Commands::Mcp at line 148; cmd_mcp_status at line 323; auth_issues block at lines 547-569 |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `crates/rightclaw-cli/src/main.rs` | `crates/rightclaw/src/mcp/detect.rs` | `rightclaw::mcp::detect::mcp_auth_status` | ✓ WIRED | main.rs:346 calls `rightclaw::mcp::detect::mcp_auth_status`; main.rs:553 calls it again in cmd_up |
| `cmd_up` in main.rs | `mcp_auth_status` | post-agent-loop, pre-process-compose-launch block | ✓ WIRED | Block at lines 547-569 follows agent loop ending at line 545, precedes `generate_process_compose` call at line 582 |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `detect.rs:mcp_auth_status` | `results: Vec<ServerStatus>` | `read_credential(credentials_path, name, &url)` reads real `~/.claude/.credentials.json` | Yes — reads actual file, no static fallback | ✓ FLOWING |
| `cmd_mcp_status` | `servers: Vec<ServerStatus>` | `mcp_auth_status(&mcp_path, &credentials_path)` with real agent path | Yes — real `.mcp.json` + real credentials | ✓ FLOWING |
| `cmd_up` auth warn block | `auth_issues: Vec<String>` | `mcp_auth_status` per agent in the already-resolved `agents` Vec | Yes — same real paths | ✓ FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `rightclaw mcp` subcommand is available | `./target/debug/rightclaw mcp --help` | Shows `status` subcommand with description | ✓ PASS |
| `rightclaw mcp status --help` describes auth states | `./target/debug/rightclaw mcp status --help` | Output contains "present / missing / expired" | ✓ PASS |
| detect module: all 9 tests pass | `cargo test -p rightclaw --lib mcp::detect` | `test result: ok. 9 passed; 0 failed` | ✓ PASS |
| workspace builds clean | `cargo build --workspace` | No errors, no unused-import warnings | ✓ PASS |
| auth warn positioned before generate_process_compose | line order check in main.rs | auth_issues block ends at 569; generate_process_compose called at 582 | ✓ PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| DETECT-01 | 33-01-PLAN.md | Operator can run `rightclaw mcp status [--agent <name>]` and see a table of MCP servers with auth state per agent (present / missing / expired) | ✓ SATISFIED | `cmd_mcp_status` function implements full table output grouped by agent; `--agent` filter with error on unknown name |
| DETECT-02 | 33-01-PLAN.md | Operator sees a non-fatal Warn during `rightclaw up` when any agent has MCP servers with missing or expired OAuth tokens | ✓ SATISFIED | auth_issues block in `cmd_up` emits `tracing::warn!("MCP auth required: ...")` non-fatally; errors in reading are also non-fatal warn |

No orphaned requirements — both DETECT-01 and DETECT-02 are claimed in the plan and implemented.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| — | — | None found | — | — |

No TODOs, FIXMEs, placeholder returns, or hardcoded empty data in modified files. Test-only imports correctly gated behind `#[cfg(test)]`.

### Human Verification Required

None. All goal-critical behaviors are verifiable programmatically. The `rightclaw up` auth warn requires an agent with an HTTP MCP server and missing credentials to observe at runtime, but the code path is fully traced and verified by inspection.

### Gaps Summary

No gaps. Phase goal is fully achieved:
- `mcp::detect` library module exists, is substantive, exports correct types, and is wired to real credential I/O.
- `rightclaw mcp status` CLI subcommand is dispatched, filters correctly, and errors on unknown agent.
- `cmd_up` auth warn block is positioned correctly (post-agent-loop, pre-process-compose), collects per-pair strings, emits a single warn line only when needed.
- All 9 detect tests pass. Pre-existing `test_status_no_running_instance` failure is unrelated to this phase (documented in MEMORY.md).
- Both DETECT-01 and DETECT-02 requirements are satisfied.

---

_Verified: 2026-04-03T15:30:00Z_
_Verifier: Claude (gsd-verifier)_
