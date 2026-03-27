# Phase 17: Memory Skill - Context

**Gathered:** 2026-03-26
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 17 delivers the agent-facing memory interface: a `rightclaw memory-server` stdio MCP server subcommand exposing 4 tools (store/recall/search/forget) backed by the Phase 16 SQLite DB, SEC-01 injection scanning on store, per-agent `.mcp.json` merge codegen in `cmd_up`, and an updated default `start_prompt` that references the memory tools.

No CLI inspection subcommand (Phase 18) in scope here.

</domain>

<decisions>
## Implementation Decisions

### Architecture: MCP over SKILL.md
- **D-01:** Memory interface is a stdio MCP server, NOT a SKILL.md bash script.
  - Rationale: structured JSON in/out, no `sqlite3` binary dep in agent sandbox, no shell escaping risk, proper typed tool responses.
  - Form: `rightclaw memory-server` as a new `Commands::MemoryServer` subcommand in `rightclaw-cli`. Single binary already installed system-wide.

### Multi-Agent Isolation
- **D-02:** Each CC session spawns its own `rightclaw memory-server` process via per-agent `.mcp.json`.
  - Each server instance opens `$HOME/memory.db` (which is `$AGENT_DIR/memory.db` under HOME override — no path injection needed).
  - Stdio = zero port conflicts across agents.
  - WAL + busy_timeout=5000ms (already set in Phase 16) handles concurrent access from CLI commands.

### MCP Library
- **D-03:** Use `rmcp` 1.3.0 (official Rust MCP SDK, `modelcontextprotocol/rust-sdk`).
  - Pattern: `#[tool]` on methods, `#[tool_router]` on impl, `#[tool_handler]` on ServerHandler impl.
  - Parameters via `Parameters<T>` with `schemars::JsonSchema` derive.
  - **CRITICAL:** tracing_subscriber MUST use `with_writer(std::io::stderr())` — default stdout writer corrupts the JSON-RPC stream and disconnects Claude Code.

### Tool API Shape
- **D-04:** Four tools with short verb names:
  - `store(content: String, tags: Option<String>)` → success/error
  - `recall(query: String)` → list of matching memory entries (tag/keyword match, active only)
  - `search(query: String)` → FTS5 full-text search results, BM25 ranked
  - `forget(id: i64)` → soft-delete (sets deleted_at, inserts memory_event)
  - Agent calls: `mcp__rightmemory__store`, `mcp__rightmemory__recall`, etc.
  - Provenance: `store()` auto-records `stored_by = $RC_AGENT_NAME` (env var already exported by shell wrapper), `source_tool = "mcp:store"`.

### .mcp.json Codegen
- **D-05:** `cmd_up` merges (not overwrites) the `rightmemory` server entry into each agent's `.mcp.json`.
  - Read existing `.mcp.json` if present, parse as JSON, inject/update `mcpServers.rightmemory` key, write back.
  - If no `.mcp.json` exists, create it with just the rightmemory entry.
  - Entry format:
    ```json
    "rightmemory": {
      "command": "rightclaw",
      "args": ["memory-server"],
      "env": {}
    }
    ```
  - Pattern: similar to existing `generate_telegram_channel_config` codegen — read optional, merge, write.

### SEC-01: Injection Scanning
- **D-06:** `store()` scans content before writing. Approach: `str::contains()` on lowercase-normalized content against a hardcoded list of ~15 multi-word injection phrases.
  - No external crate — none viable (only candidate has 27 downloads).
  - Pattern source: OWASP LLM01:2025 + Rebuff heuristics.
  - Conservative list (multi-word phrases only — avoids false positives):
    - `"ignore previous instructions"`, `"ignore all previous instructions"`
    - `"disregard previous instructions"`, `"disregard your training"`
    - `"reveal your system prompt"`, `"bypass safety"`
    - `"jailbreak"`, `"developer mode enabled"`
    - LLM tokenizer artifacts: `"<|im_start|>"`, `"[inst]"`, `"<|system|>"`
  - On match: return `CallToolResult::error("content rejected: potential prompt injection")`. Write is NOT persisted.
  - Single words (`"forget"`, `"override"`) are explicitly NOT in the list — too high false positive rate.
  - Research file: `.planning/phases/17-memory-skill/17-SEC01-RESEARCH.md`

### Default Start Prompt Update (deferred from Phase 16, D-07)
- **D-07:** Update default `start_prompt` in `system_prompt.rs` from `"You are starting."` to:
  `"You are starting. Use mcp__rightmemory__store/recall/search/forget to manage persistent memory."`
  This is a small change folded into Phase 17 since the memory tools now exist.

### New Cargo Crate
- **D-08:** Add `rmcp` and `schemars` to workspace dependencies.
  - `rmcp` goes in both workspace root Cargo.toml AND `crates/rightclaw-cli/Cargo.toml` (binary crate uses it for the server subcommand).
  - `schemars` needed for `JsonSchema` derive on tool parameters.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 16 Foundation (what we build on)
- `.planning/phases/16-db-foundation/16-CONTEXT.md` — schema topology (D-02), open_db API
- `crates/rightclaw/src/memory/mod.rs` — open_db signature and MemoryError type
- `crates/rightclaw/src/memory/sql/v1_schema.sql` — table/column names for SQL queries

### SEC-01 Research
- `.planning/phases/17-memory-skill/17-SEC01-RESEARCH.md` — injection pattern list, rmcp API examples, false positive analysis

### Existing Codegen Patterns to Follow
- `crates/rightclaw/src/codegen/skills.rs` — install_builtin_skills pattern (include_str!, write to agent dir)
- `crates/rightclaw/src/codegen/telegram.rs` — generate_telegram_channel_config pattern (read optional, merge, write JSON)
- `crates/rightclaw-cli/src/main.rs` cmd_up loop (lines ~316-407) — per-agent scaffold step pattern; new step 11 for .mcp.json merge
- `crates/rightclaw/src/codegen/system_prompt.rs` — default start_prompt location

### Requirements
- `.planning/REQUIREMENTS.md` — SKILL-01 through SKILL-05, SEC-01 (this phase)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `rightclaw::memory::open_db(&agent_path)` — already exists, opens WAL-mode DB with migrations applied
- `RC_AGENT_NAME` env var — already exported by shell wrapper (Phase 11), available as `std::env::var("RC_AGENT_NAME")` in memory-server process
- `generate_telegram_channel_config()` in `telegram.rs` — JSON merge pattern to adapt for .mcp.json

### Established Patterns
- `include_str!` for embedded content (skills.rs) — NOT needed for MCP server (it's runtime code, not embedded)
- Per-agent loop step pattern in `cmd_up` — add step 11 after step 10 (open_db)
- `miette::miette!("failed to ... for '{agent}': {e:#}")` — error format for codegen failures

### Integration Points
- `Commands` enum in `main.rs` — add `MemoryServer` variant (no args needed; reads `$HOME/memory.db`)
- `cmd_up` per-agent loop — add step 11: `generate_mcp_config(&agent.path)?`
- New codegen file: `crates/rightclaw/src/codegen/mcp_config.rs` — `generate_mcp_config(agent_path: &Path)`
- `system_prompt.rs:16` — update default start_prompt string

</code_context>

<specifics>
## Specific Notes

- rmcp 1.3.0 is the correct version (published 2026-03-26). Import pattern: `use rmcp::{ServerHandler, tool, tool_router, tool_handler};`
- `schemars` crate needed alongside rmcp for `JsonSchema` derive on Parameters structs
- Server main pattern: `rmcp::service::stdio().serve(MemoryServer).await?.waiting().await?`
- `SKILL-05` (install rightmemory built-in) is now OBSOLETE — there is no SKILL.md to install. The MCP server replaces the skill approach entirely. This requirement is satisfied by the .mcp.json codegen (D-05).

</specifics>

<deferred>
## Deferred Ideas

- Phase 18: `rightclaw memory list/search/delete/stats` CLI — uses same rusqlite directly, no MCP
- v2.4: Vector/semantic search via sqlite-vec extension
- v2.4: Memory eviction policy (expires_at, importance threshold columns already in schema)
- v2.4: Cross-agent memory sharing via named shared DB

</deferred>

---

*Phase: 17-memory-skill*
*Context gathered: 2026-03-26*
