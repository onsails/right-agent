# Phase 16: DB Foundation - Context

**Gathered:** 2026-03-26
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 16 delivers the Rust infrastructure layer for per-agent SQLite memory: the `memory` module in `rightclaw-core`, DB creation wired into `cmd_up`, V1 schema (append-only, FTS5), embedded migrations, a `rightclaw doctor` check for `sqlite3`, and removal of the stale `MEMORY.md` convention from the codebase.

No skill (Phase 17) and no CLI subcommand (Phase 18) are in scope here.

</domain>

<decisions>
## Implementation Decisions

### DB Creation Failure
- **D-01:** Fatal error — `rightclaw up` must fail if `memory.db` cannot be created for any agent. Same error pattern as `settings.json` generation: `"failed to open memory database for '{agent}': {err:#}"`.

### Schema Topology
- **D-02:** Two-table design for architectural correctness (not single table + soft-delete):
  - `memories(id INTEGER PK, content TEXT NOT NULL, tags TEXT, stored_by TEXT, source_tool TEXT, created_at TEXT, deleted_at TEXT, expires_at TEXT, importance REAL DEFAULT 0.5)`
  - `memory_events(id INTEGER PK, memory_id INTEGER, event_type TEXT, actor TEXT, created_at TEXT)` — append-only audit log; SQLite ABORT triggers prevent UPDATE/DELETE
  - `memories_fts` — FTS5 virtual table, content=memories, auto-synced via triggers
- **D-03:** Schema managed via `rusqlite_migration 2.5` with `user_version` pragma. `to_latest()` called on every DB open — idempotent, safe on repeated `rightclaw up`.

### SEC-02 Enforcement
- **D-04:** Dropped as an explicit requirement. Enforced by architecture: the `memory` module has no code paths to MEMORY.md; they are unrelated systems. No grep test needed.

### MEMORY.md Cleanup (folded from discovery)
- **D-05:** Remove `memory_path` field from `AgentDef` — it is discovered but never consumed anywhere in the codebase (dead code).
- **D-06:** Remove `optional_file(&path, "MEMORY.md")` scan from `discovery.rs`.
- **D-07:** Change default `start_prompt` in `system_prompt.rs` from `"You are starting. Read your MEMORY.md to restore context."` to `"You are starting."` — MEMORY.md is CC-native; we no longer push agents toward it. Phase 17 will update this to reference `/recall`.
- **D-08:** Remove the `discovery_tests.rs` assertion that `memory_path.is_some()` when MEMORY.md exists in an agent dir.

### Rationale for MEMORY.md removal
IronClaw analysis (cloned 2026-03-26) confirms: SQLite chunks are never auto-injected into system prompts. Only fixed named files (MEMORY.md, daily logs, SOUL.md) are always-on. CC handles MEMORY.md natively — agents can write it via CC file tools, CC injects it automatically. We have no reason to manage it ourselves. The `memory_path` field was dead code. The default start_prompt was pointing agents to a flat-file pattern we're superseding with SQLite.

### Doctor Check
- **D-09:** `rightclaw doctor` adds a Warn (non-fatal) check: `sqlite3` binary in PATH. No fix suggestion — sqlite3 is available on all standard macOS and Linux installs. Pattern: existing `check_binary()` helper in `doctor.rs`.

### Module Placement
- **D-10:** New module at `crates/rightclaw/src/memory/` — consistent with `codegen/`, `agent/`, `runtime/` layout. Four files: `error.rs`, `migrations.rs`, `store.rs`, `mod.rs`. No new workspace crate — stays in `rightclaw` library crate.

### Cargo Dependencies
- **D-11:** Add to workspace `Cargo.toml`:
  - `rusqlite = { version = "0.39", features = ["bundled"] }` — bundled embeds SQLite 3.51.1, eliminates system SQLite variability
  - `rusqlite_migration = "2.5"` — migration management via `user_version` pragma

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` — DB-01 through DB-04, SEC-02, SEC-03, DOCTOR-01 scoped to this phase
- `.planning/research/SUMMARY.md` — executive summary of stack + architecture decisions
- `.planning/research/ARCHITECTURE.md` — module structure, DB location, build order
- `.planning/research/STACK.md` — rusqlite version rationale, sqlx rejection

### Existing Code Patterns to Follow
- `crates/rightclaw/src/codegen/skills.rs` — `install_builtin_skills()` pattern: `include_str!`, write to agent dir, create-if-absent idiom
- `crates/rightclaw/src/doctor.rs` — `DoctorCheck` struct + `check_binary()` helper; Warn/Fail/Pass pattern
- `crates/rightclaw-cli/src/main.rs` lines 316-405 — `cmd_up` per-agent loop; DB creation is step 10 (after settings.local.json)
- `crates/rightclaw/src/agent/discovery.rs` — `optional_file()` helper; struct construction pattern
- `crates/rightclaw/src/agent/types.rs` — `AgentDef` struct (remove `memory_path` here)
- `crates/rightclaw/src/codegen/system_prompt.rs` — default start_prompt location (line 16)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `check_binary(name, url)` in `doctor.rs` — direct reuse for `sqlite3` doctor check
- `optional_file(&path, filename)` helper in `discovery.rs` — pattern for path handling
- `miette::miette!("failed to ... for '{agent}': {e:#}")` — error format used throughout `cmd_up`

### Established Patterns
- Per-agent loop in `cmd_up`: numbered steps, fatal error propagates with `?`, `tracing::debug!` for success
- Library crate (`rightclaw`) exports modules used by CLI crate (`rightclaw-cli`)
- `include_str!` for embedded file content (skills.rs) — same approach for embedded SQL migrations
- `serde_json`/`miette` already in workspace deps; no new error handling crates needed

### Integration Points
- Step 10 in `cmd_up` per-agent loop: call `rightclaw::memory::open_or_create_db(&agent.path)?`
- `AgentDef` struct: remove `memory_path` field and all struct literal sites (init.rs:80, init.rs:155, main.rs:731, telegram.rs:88, and all test fixtures)
- `run_doctor(home)` in `doctor.rs`: add `sqlite3` check to existing checks vec
- `Cargo.toml` workspace deps: add rusqlite + rusqlite_migration

</code_context>

<specifics>
## Specific Ideas

- **IronClaw reference** (analyzed 2026-03-26 at /tmp/ironclaw): SQLite chunks are NEVER auto-injected into system prompts. Only fixed named documents (MEMORY.md, daily logs) are always-on. SQLite is exclusively for on-demand search via tool calls. This confirms RightClaw's approach: SQLite memory = on-demand via `/recall`/`/search` only.
- The default start_prompt will be updated again in Phase 17 to `"You are starting. Use /recall to restore memory from previous sessions."` once the skill exists.

</specifics>

<deferred>
## Deferred Ideas

- Phase 17: Update default start_prompt to reference `/recall` once memory skill is installed
- Phase 17: Injection scanning for SEC-01 (needs dedicated research pass on Rust regex patterns before implementing — flagged by research)
- v2.4: `expires_at`/`importance` columns in schema from day 1 (included in D-02) but eviction logic deferred
- v2.4: Cross-agent memory sharing via named shared DB (out of scope for v2.3)

</deferred>

---

*Phase: 16-db-foundation*
*Context gathered: 2026-03-26*
