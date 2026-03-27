---
phase: 17-memory-skill
verified: 2026-03-26T23:00:00Z
status: passed
score: 4/4 success criteria verified
re_verification: null
gaps: []
human_verification:
  - test: "Launch rightclaw up with a test agent, verify .mcp.json is written with rightmemory entry"
    expected: "Agent dir/.mcp.json contains mcpServers.rightmemory.command = 'rightclaw'"
    why_human: "Integration path through cmd_up exercised only at runtime; unit tests cover generate_mcp_config in isolation"
  - test: "Attach a Claude Code session via rightclaw and confirm mcp__rightmemory__store tool is visible"
    expected: "Claude Code lists 4 rightmemory tools in /tools or via tab completion"
    why_human: "MCP server stdio binding with Claude Code cannot be verified statically"
---

# Phase 17: Memory Skill Verification Report

**Phase Goal:** Agents can store, retrieve, search, and forget memories via MCP tools backed by per-agent SQLite
**Verified:** 2026-03-26
**Status:** PASSED
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (from ROADMAP.md Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Agent session has `mcp__rightmemory__store/recall/search/forget` tools available via MCP server launched on every `rightclaw up` | VERIFIED | `Commands::MemoryServer` in main.rs; step 11 calls `generate_mcp_config` per agent; `.mcp.json` injects `rightmemory` entry with `command: "rightclaw", args: ["memory-server"]` |
| 2 | `store` tool records `stored_by` (agent name) and `source_tool` provenance automatically — no manual input required | VERIFIED | `memory_server.rs:73-74` passes `self.agent_name` (from `RC_AGENT_NAME` env) and `"mcp:store"` as provenance; store_memory inserts both into the memories row |
| 3 | `forget` tool excludes the entry from all subsequent `recall` and `search` results while preserving the audit row in `memory_events` | VERIFIED | `forget_memory` sets `deleted_at`; SQL queries in `recall_memories` and `search_memories` both have `WHERE deleted_at IS NULL`; 2 tests confirm post-forget exclusion |
| 4 | `store` tool rejects entries matching prompt injection patterns and returns an error message — the write is not persisted | VERIFIED | `has_injection()` is the first call in `store_memory()`; returns `Err(InjectionDetected)` before any INSERT; `memory_server.rs:79-82` maps this to `McpError::invalid_params`; test confirms count stays 0 |

**Score:** 4/4 success criteria verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw/src/memory/guard.rs` | Injection detection with 15 patterns | VERIFIED | `pub fn has_injection` + `pub static INJECTION_PATTERNS` (15 entries); `to_lowercase()` once; 15 tests (10 detection, 5 false-positive) |
| `crates/rightclaw/src/memory/store.rs` | CRUD operations: store/recall/search/forget + MemoryEntry | VERIFIED | All 4 functions exported; `MemoryEntry` struct with 7 fields; guard called as first line in `store_memory` |
| `crates/rightclaw/src/memory/store_tests.rs` | Min 80 lines, comprehensive tests | VERIFIED | 242 lines; 17 tests covering all paths: injection rejection, soft-delete exclusion, FTS5 search, forget event audit |
| `crates/rightclaw/src/memory/mod.rs` | `open_connection()` returning live `Connection` | VERIFIED | `pub fn open_connection` present; returns `Result<rusqlite::Connection, MemoryError>`; same WAL+migration logic as `open_db`; 3 dedicated tests |
| `crates/rightclaw/src/memory/error.rs` | `InjectionDetected` and `NotFound(i64)` variants | VERIFIED | Both variants present with correct thiserror attributes |
| `crates/rightclaw-cli/src/memory_server.rs` | MCP stdio server with 4 tools | VERIFIED | `pub async fn run_memory_server`; `#[tool_router]` + `#[tool_handler]` macros; 4 tool methods calling store layer; `with_writer(std::io::stderr)` confirmed |
| `crates/rightclaw/src/codegen/mcp_config.rs` | `.mcp.json` merge codegen | VERIFIED | `pub fn generate_mcp_config`; non-destructive merge; `"rightmemory"` + `"memory-server"` present; 6 unit tests |
| `crates/rightclaw-cli/src/main.rs` | `Commands::MemoryServer` subcommand | VERIFIED | Variant present; dispatched BEFORE tracing init via `matches!` guard; step 11 wires `generate_mcp_config` in cmd_up loop |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `memory/store.rs` | `memory/guard.rs` | `guard::has_injection()` first line of `store_memory` | WIRED | `store.rs:29` — `if guard::has_injection(content)` |
| `memory/store.rs` | `memories` / `memory_events` / `memories_fts` tables | SQL INSERT/SELECT/UPDATE queries | WIRED | Lines 32-40 (store), 51-58 (recall), 83-90 (search), 130-136 (forget) |
| `memory/mod.rs` | `memory/store.rs` | `pub mod store` re-export | WIRED | Line 3: `pub mod store;` |
| `memory_server.rs` | `memory/store.rs` | calls store/recall/search/forget | WIRED | `rightclaw::memory::store::store_memory` (line 69), `recall_memories` (96), `search_memories` (113), `forget_memory` (131) |
| `memory_server.rs` | `memory/mod.rs` | `open_connection()` | WIRED | Line 183: `rightclaw::memory::open_connection(&home)` |
| `main.rs` | `codegen/mcp_config.rs` | step 11 cmd_up calls `generate_mcp_config` | WIRED | Line 424: `rightclaw::codegen::generate_mcp_config(&agent.path)?` |
| `main.rs` | `memory_server.rs` | `Commands::MemoryServer` dispatches `run_memory_server` | WIRED | Lines 93-95: early dispatch before tracing init |
| `codegen/mod.rs` | `codegen/mcp_config.rs` | `pub mod mcp_config` + re-export | WIRED | Lines 2, 11 of mod.rs |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| SKILL-01 | 17-01, 17-02 | Agent can store memory via `/remember` — provenance auto-recorded | SATISFIED | `store_memory()` + `store` MCP tool captures `agent_name` from `RC_AGENT_NAME`; `source_tool = "mcp:store"` |
| SKILL-02 | 17-01, 17-02 | Agent can look up memories via `/recall` — tag/keyword lookup | SATISFIED | `recall_memories()` + `recall` MCP tool; LIKE search on tags AND content |
| SKILL-03 | 17-01, 17-02 | Agent can full-text search via `/search` — FTS5 BM25 ranking | SATISFIED | `search_memories()` uses `memories_fts MATCH` + `ORDER BY bm25(memories_fts)` |
| SKILL-04 | 17-01, 17-02 | Agent can soft-delete via `/forget` — entry excluded, audit row preserved | SATISFIED | `forget_memory()` sets `deleted_at`; inserts `"forget"` event in `memory_events`; recall/search exclude via `WHERE deleted_at IS NULL` |
| SKILL-05 | 17-02 | `rightmemory` skill installed as built-in on every `rightclaw up` | SATISFIED (REDEFINED) | Design decision D-05/CONTEXT.md line 135: SKILL.md approach obsolete; replaced by `.mcp.json` codegen. Step 11 in cmd_up writes `mcpServers.rightmemory` entry. REQUIREMENTS.md description predates this decision. |
| SEC-01 | 17-01, 17-02 | `/remember` scans for prompt injection before writing; rejects on match | SATISFIED | 15-pattern list in `INJECTION_PATTERNS`; `has_injection()` is first call in `store_memory()`; MCP error returned without persisting; 2 tests verify rejection + no-insert |

**Note on SKILL-05:** The requirement description in REQUIREMENTS.md references a `rightmemory/SKILL.md` file installed by `install_builtin_skills()`. No such file exists and `install_builtin_skills()` was not modified in Phase 17. However, CONTEXT.md (authoritative design doc) explicitly marks this interpretation "OBSOLETE" and states the requirement is satisfied by `.mcp.json` codegen (D-05). The ROADMAP.md success criteria — which take verification priority — do not mention a skill file. The MCP approach (step 11 in cmd_up + `generate_mcp_config`) directly delivers the agent-facing memory interface that SKILL-05 intended. REQUIREMENTS.md should be updated to reflect the final implementation.

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| None detected | — | — | — |

Checked all 8 new/modified files for: TODO/FIXME/PLACEHOLDER, `return null/[]/{}`, hardcoded empty data, console-only handlers. No stubs found. All 4 MCP tool methods call real SQLite operations.

### Human Verification Required

#### 1. End-to-End MCP Server Launch

**Test:** Run `rightclaw up` in a test agent directory, then inspect the generated `.mcp.json`.
**Expected:** File contains `mcpServers.rightmemory.command = "rightclaw"` and `args = ["memory-server"]`.
**Why human:** Integration path through cmd_up exercised only at runtime; the unit test in `mcp_config.rs` tests `generate_mcp_config` in isolation.

#### 2. Claude Code Tool Discovery

**Test:** Attach a Claude Code session to a running agent, confirm `mcp__rightmemory__store/recall/search/forget` tools are listed.
**Expected:** All 4 tools appear in Claude Code's tool list with their schemars-derived descriptions.
**Why human:** MCP stdio handshake between rightclaw binary and CC runtime cannot be verified statically; requires live process.

### Test Results

| Suite | Tests | Status |
|-------|-------|--------|
| `rightclaw` lib (memory::guard, memory::store, memory::mod, codegen::mcp_config, codegen::system_prompt) | 214 | PASSED |
| `rightclaw-cli` integration | 19 passed, 1 failed | PRE-EXISTING FAILURE: `test_status_no_running_instance` (HTTP connection refused; documented in MEMORY.md) |
| Workspace build | — | CLEAN — `cargo build --workspace` exits 0, 0 warnings |

### Gaps Summary

No gaps. All 4 ROADMAP.md success criteria are verified in the codebase:

1. `.mcp.json` codegen (step 11 in cmd_up) injects `rightmemory` MCP server entry per agent — confirmed wired.
2. `stored_by` auto-populated from `RC_AGENT_NAME` env var in `run_memory_server` — confirmed wired.
3. Soft-delete exclusion in `recall_memories`/`search_memories` — confirmed via `WHERE deleted_at IS NULL` + passing tests.
4. Injection guard is the first call in `store_memory` — confirmed wired; returns `InjectionDetected` without writing.

The only open item is a REQUIREMENTS.md description mismatch for SKILL-05 that reflects a pre-decision state. The implementation is correct; the requirement text needs a documentation update.

---

_Verified: 2026-03-26_
_Verifier: Claude (gsd-verifier)_
