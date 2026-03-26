---
phase: 17-memory-skill
plan: "02"
subsystem: mcp
tags: [rmcp, mcp, sqlite, memory, codegen, stdio, schemars]

# Dependency graph
requires:
  - phase: 17-01
    provides: store_memory/recall_memories/search_memories/forget_memory, open_connection(), MemoryError::InjectionDetected/NotFound
  - phase: 16-db-foundation
    provides: SQLite schema (memories, memory_events, memories_fts), WAL mode, migrations
provides:
  - run_memory_server() — MCP stdio server with store/recall/search/forget tools
  - MemoryServer struct with Arc<Mutex<Connection>> for async tool handlers
  - generate_mcp_config() — non-destructive .mcp.json merge writing mcpServers.rightmemory
  - Commands::MemoryServer subcommand (kebab-case: memory-server)
  - cmd_up step 11: per-agent .mcp.json generation on every launch
  - Default start_prompt updated to reference mcp__rightmemory tools
affects: []

# Tech tracking
tech-stack:
  added:
    - "rmcp 1.3.0 (macros feature) — official Anthropic MCP SDK for stdio server"
    - "schemars 1.1 — JSON Schema generation for tool parameter structs"
  patterns:
    - "rmcp #[tool_router] + #[tool_handler] macros for ServerHandler implementation"
    - "Arc<Mutex<Connection>> for sharing rusqlite Connection across async tool handlers"
    - "ServerInfo::new(capabilities).with_instructions() — non_exhaustive struct init pattern"
    - ".mcp.json merge pattern: read-or-default, parse, insert key, write back"
    - "MemoryServer early dispatch before tracing_subscriber::fmt() to protect stdout"

key-files:
  created:
    - crates/rightclaw-cli/src/memory_server.rs
    - crates/rightclaw/src/codegen/mcp_config.rs
  modified:
    - Cargo.toml
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw-cli/src/main.rs
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/system_prompt.rs
    - crates/rightclaw/src/codegen/system_prompt_tests.rs

key-decisions:
  - "Use ServerInfo::new().with_instructions() instead of struct literal — InitializeResult is #[non_exhaustive] in rmcp 1.3"
  - "run_memory_server() returns miette::Result<()> — no anyhow in CLI crate, miette is the standard"
  - "Add rusqlite as direct dep of rightclaw-cli — needed for Arc<Mutex<Connection>> type"
  - "cargo update required before build — rmcp-macros 1.3.0 not in local index cache"

patterns-established:
  - "MCP server dispatch: matches! check BEFORE tracing_subscriber init to avoid stdout pollution"
  - ".mcp.json codegen: read-or-empty, merge mcpServers.rightmemory, preserve all other keys"

requirements-completed: [SKILL-01, SKILL-02, SKILL-03, SKILL-04, SKILL-05, SEC-01]

# Metrics
duration: 8min
completed: 2026-03-26
---

# Phase 17 Plan 02: MCP Memory Server Summary

**rmcp 1.3 stdio MCP server with 4 tools (store/recall/search/forget) wired to Phase 17 SQLite layer, per-agent .mcp.json codegen, and default start_prompt updated with tool references**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-03-26T22:24:33Z
- **Completed:** 2026-03-26T22:32:08Z
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments

- `memory_server.rs`: MCP stdio server using `#[tool_router]` + `#[tool_handler]` macros; 4 tools calling Phase 17 CRUD layer; `Arc<Mutex<Connection>>` for async-safe SQLite access; tracing to stderr only
- `mcp_config.rs`: `generate_mcp_config()` merges `mcpServers.rightmemory` into existing `.mcp.json` or creates it; preserves all other keys; 6 unit tests
- `cmd_up` step 11: calls `generate_mcp_config(&agent.path)` per agent after DB init
- `Commands::MemoryServer`: clap subcommand mapping to `memory-server`, dispatched before tracing init
- Default `start_prompt` now references `mcp__rightmemory__store/recall/search/forget`
- All 214 lib tests pass, 0 clippy warnings

## Task Commits

Each task was committed atomically:

1. **Task 1: MCP server module + rmcp dependencies** - `9d51a23` (feat)
2. **Task 2: .mcp.json codegen in cmd_up + start_prompt update** - `8ac1f2a` (feat)

## Files Created/Modified

- `crates/rightclaw-cli/src/memory_server.rs` — MCP stdio server, 4 tools, Arc<Mutex<Connection>>, run_memory_server()
- `crates/rightclaw/src/codegen/mcp_config.rs` — generate_mcp_config(), 6 unit tests
- `Cargo.toml` — added rmcp 1.3 (macros feature) + schemars 1.1 workspace deps
- `crates/rightclaw-cli/Cargo.toml` — added rmcp/schemars/rusqlite dependencies
- `crates/rightclaw-cli/src/main.rs` — Commands::MemoryServer variant + early dispatch + step 11
- `crates/rightclaw/src/codegen/mod.rs` — pub mod mcp_config + pub use generate_mcp_config
- `crates/rightclaw/src/codegen/system_prompt.rs` — default start_prompt includes mcp__rightmemory tools
- `crates/rightclaw/src/codegen/system_prompt_tests.rs` — added mcp__rightmemory assertion

## Decisions Made

- `ServerInfo` struct is `#[non_exhaustive]` in rmcp 1.3 — used `ServerInfo::new(capabilities).with_instructions(...)` builder pattern instead of struct literal
- `run_memory_server()` returns `miette::Result<()>` — CLI crate doesn't have anyhow; miette is already the error type for the binary
- `rusqlite` added as direct dependency of `rightclaw-cli` — needed to name `rusqlite::Connection` in the `Arc<Mutex<T>>` field type
- `cargo update` was required to resolve `rmcp-macros 1.3.0` — the local crates.io index was stale and didn't include the entry yet

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] cargo update needed for rmcp-macros 1.3.0**
- **Found during:** Task 1 (cargo build)
- **Issue:** `rmcp-macros = "^1.3.0"` unsatisfied — local crates.io index didn't have the v1.3.0 entry
- **Fix:** Ran `cargo update` to refresh the index; resolved immediately
- **Files modified:** Cargo.lock
- **Verification:** Build succeeded after update
- **Committed in:** `9d51a23` (Task 1 commit)

**2. [Rule 1 - Bug] ServerInfo struct literal rejected due to #[non_exhaustive]**
- **Found during:** Task 1 (cargo build — E0639)
- **Issue:** `InitializeResult` (aliased as `ServerInfo`) is marked `#[non_exhaustive]` in rmcp 1.3.0; struct literal syntax rejected outside the crate
- **Fix:** Used `ServerInfo::new(capabilities).with_instructions(...)` builder pattern
- **Files modified:** `crates/rightclaw-cli/src/memory_server.rs`
- **Verification:** Compiled cleanly, matches official rmcp API
- **Committed in:** `9d51a23` (Task 1 commit)

**3. [Rule 3 - Blocking] rusqlite not in CLI crate Cargo.toml**
- **Found during:** Task 1 (cargo build — E0433)
- **Issue:** `rusqlite::Connection` used in `Arc<Mutex<rusqlite::Connection>>` field but `rusqlite` not a direct dep of `rightclaw-cli`
- **Fix:** Added `rusqlite = { workspace = true }` to `crates/rightclaw-cli/Cargo.toml`
- **Files modified:** `crates/rightclaw-cli/Cargo.toml`
- **Verification:** Compiled cleanly
- **Committed in:** `9d51a23` (Task 1 commit)

**4. [Rule 1 - Bug] anyhow unavailable in CLI crate**
- **Found during:** Task 1 (cargo build — E0433 on anyhow::anyhow!)
- **Issue:** Plan specified `anyhow::Result<()>` for `run_memory_server` return type, but CLI crate uses miette; anyhow not a dep
- **Fix:** Changed return type to `miette::Result<()>` and converted errors with `miette::miette!()` macro
- **Files modified:** `crates/rightclaw-cli/src/memory_server.rs`
- **Verification:** Compiled cleanly, consistent with CLI error handling conventions
- **Committed in:** `9d51a23` (Task 1 commit)

**5. [Rule 1 - Bug] rmcp macro imports at wrong path**
- **Found during:** Task 1 (cargo build — E0432 for `tool`, `tool_handler`, `tool_router`)
- **Issue:** Research doc showed `use rmcp::{tool, tool_handler, tool_router}` but macros are re-exported via `rmcp_macros::*` only when macros feature enabled; initial build had `default-features = false` without `macros` feature
- **Fix:** Added `macros` to rmcp features in Cargo.toml; changed `Parameters` import to `rmcp::handler::server::wrapper::Parameters` (canonical path per compiler hint)
- **Files modified:** `Cargo.toml`, `crates/rightclaw-cli/src/memory_server.rs`
- **Verification:** Compiled cleanly after both fixes
- **Committed in:** `9d51a23` (Task 1 commit)

---

**Total deviations:** 5 auto-fixed (3 blocking dep/import issues, 2 API behavior bugs)
**Impact on plan:** All auto-fixes were compile errors immediately surfaced by cargo. No scope change. Fixes follow project conventions.

## Issues Encountered

None beyond the five auto-fixed deviations above. All were caught at compile time.

## User Setup Required

None — no external service configuration required. The `rightclaw` binary must be in PATH for the `.mcp.json` `command: "rightclaw"` entry to work, but that is already satisfied by any standard install.

## Known Stubs

None — all 4 tools call real SQLite operations from Phase 17-01. No placeholder data.

## Next Phase Readiness

- Phase 17 complete: memory library (Plan 01) + MCP server (Plan 02) both shipped
- Agents launched via `rightclaw up` will have `mcp__rightmemory__*` tools in their CC session
- `rightclaw memory-server` subcommand is live; Claude Code discovers it via `.mcp.json`
- No blockers for v2.3 milestone close

---
*Phase: 17-memory-skill*
*Completed: 2026-03-26*
