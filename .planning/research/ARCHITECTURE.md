# Architecture Research

**Domain:** v2.3 Memory System — per-agent SQLite memory integration
**Researched:** 2026-03-26
**Confidence:** HIGH (direct codebase inspection + verified library research)

## System Overview

```
┌────────────────────────────────────────────────────────────────┐
│                    rightclaw-cli (binary)                       │
│  Commands: up | down | status | list | init | doctor | memory  │
└──────────────┬─────────────────────────────────────────────────┘
               │  calls into
┌──────────────▼─────────────────────────────────────────────────┐
│                  rightclaw-core (library crate)                 │
│                                                                  │
│  agent/        AgentDef, AgentConfig, discover_agents()         │
│  codegen/      shell_wrapper, settings, skills, system_prompt   │
│  runtime/      state.rs, pc_client.rs, deps.rs                 │
│  init.rs       rightclaw home + default agent scaffolding       │
│  doctor.rs     dependency + config validation                   │
│  memory/       [NEW] open_db(), MemoryStore, migrations         │
└──────────────┬─────────────────────────────────────────────────┘
               │
    ┌──────────▼──────────────────────────────────────────┐
    │        Per-Agent HOME: ~/.rightclaw/agents/<name>/   │
    │                                                       │
    │  IDENTITY.md   SOUL.md   AGENTS.md   agent.yaml      │
    │  memory.db        [NEW] SQLite DB — flat in agent/   │
    │  .claude/                                             │
    │    settings.json    (sandbox config, regen on up)     │
    │    settings.local.json  (user-editable, create-once)  │
    │    .credentials.json -> host ~/.claude/.credentials   │
    │    skills/                                            │
    │      rightskills/SKILL.md                             │
    │      rightcron/SKILL.md                               │
    │      rightmemory/SKILL.md   [NEW]                    │
    │      installed.json                                   │
    └───────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Status |
|-----------|---------------|--------|
| `rightclaw-core/src/memory/` | DB open/init, migrations, CRUD queries, vacuum | NEW |
| `rightclaw-core/src/codegen/skills.rs` | Add `rightmemory/SKILL.md` to `install_builtin_skills()` | MODIFIED |
| `rightclaw-cli/src/main.rs` | Add `Commands::Memory` + `cmd_memory()` | MODIFIED |
| `skills/rightmemory/SKILL.md` | Embedded skill: instructs CC to call `sqlite3` directly | NEW |
| `~/.rightclaw/agents/<n>/memory.db` | Per-agent SQLite database, flat in agent dir root | NEW FILE |

---

## DB File Location

**Decision: `~/.rightclaw/agents/<name>/memory.db` — flat in the agent directory root.**

Rationale:
- Agent dir is `$HOME` under the HOME override. `memory.db` at `$HOME/memory.db` is the natural path the skill can always reference as `$HOME/memory.db` without hardcoded absolute paths.
- Avoids nesting inside `.claude/` — that dir is managed by CC and partially scaffold-generated (settings.json regenerates on every `up`). Future scaffold changes must not touch the DB.
- `rightclaw memory` CLI resolves the path as `home.join("agents").join(name).join("memory.db")` — no ambiguity.
- Sandbox `allowWrite` already includes the agent path. No new sandbox entries needed.
- Consistent with `crons/` directory (also flat in agent root, not inside `.claude/`).

**Alternative rejected:** `.claude/memory.db` — co-located with scaffold-generated files; risk of accidental interference or deletion.

---

## Database Schema

```sql
-- migrations.rs: M::up() string
CREATE TABLE IF NOT EXISTS memories (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    key        TEXT    NOT NULL,
    value      TEXT    NOT NULL,
    tags       TEXT    NOT NULL DEFAULT '',
    source     TEXT    NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_key ON memories(key);
CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories(tags);
```

Design notes:
- `key` is non-unique — multiple entries per slot accumulate with timestamps, providing full audit trail.
- `tags` is comma-separated string in a single column. No join table. Queried with `LIKE '%tag%'`. Sufficient for low-volume per-agent memory in v2.3.
- `user_version` pragma tracks migration state via `rusqlite_migration` — no extra tracking table, just an integer at a fixed offset in the SQLite file header.
- No FTS5 in v2.3. Plain `LIKE '%query%'` on `key || value || tags` is adequate for typical agent memory volumes (hundreds to low thousands of rows). FTS5 is a v2.4 candidate.
- WAL mode enabled on open: `PRAGMA journal_mode=WAL`. Allows concurrent readers (CLI inspection) without blocking agent writes.

---

## New Module: `rightclaw-core/src/memory/`

```
src/memory/
├── mod.rs          pub open_db(), pub use MemoryStore, pub use MemoryError
├── migrations.rs   Migrations::new(vec![M::up(SCHEMA_SQL)])
├── store.rs        MemoryStore struct + insert/list/search/delete/vacuum
└── error.rs        MemoryError (thiserror derive)
```

### `open_db(agent_path: &Path) -> Result<Connection, MemoryError>`

1. Opens `agent_path/memory.db` (SQLite creates if absent).
2. Sets WAL: `conn.pragma_update(None, "journal_mode", "WAL")?`
3. Runs `MIGRATIONS.to_latest(&mut conn)?` — idempotent, no-op when schema is current.
4. Returns `Connection`.

Called from:
- `cmd_up` per-agent scaffold loop (step 10, after `install_builtin_skills`) — ensures DB schema exists before agent starts.
- `cmd_memory` — opens DB for CLI read/write operations.

Note: `open_db` is sync. No `tokio::spawn_blocking` needed. `cmd_up` is async only for process-compose launch; the scaffold loop before launch is effectively sync and calls sync functions throughout (`install_builtin_skills`, `generate_settings`, etc.). Same pattern here.

### `MemoryStore`

```rust
pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    pub fn open(agent_path: &Path) -> Result<Self, MemoryError>;
    pub fn insert(&self, key: &str, value: &str, tags: &[&str], source: &str) -> Result<i64, MemoryError>;
    pub fn list(&self, limit: usize) -> Result<Vec<MemoryRow>, MemoryError>;
    pub fn search(&self, query: &str) -> Result<Vec<MemoryRow>, MemoryError>;
    pub fn delete(&self, id: i64) -> Result<bool, MemoryError>;
    pub fn vacuum(&self) -> Result<(), MemoryError>;
}

pub struct MemoryRow {
    pub id: i64,
    pub key: String,
    pub value: String,
    pub tags: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
}
```

`MemoryError` uses `thiserror` — consistent with library crate convention in `rightclaw-core` (same as `error.rs` in other modules).

---

## CLI: `rightclaw memory` Subcommand

New `Commands::Memory` variant in `rightclaw-cli/src/main.rs`:

```
rightclaw memory list   [--agent <name>] [--limit N]
rightclaw memory search <query> [--agent <name>]
rightclaw memory delete <id> [--agent <name>]
rightclaw memory vacuum [--agent <name>]
```

`--agent` is required when multiple agents exist and no default can be inferred. For single-agent setups, default to the only discovered agent (ergonomic).

`cmd_memory()` is a synchronous fn (no async — no process-compose interaction). It:
1. Resolves agent path from `home/agents/<name>/`.
2. Opens `MemoryStore::open(&agent_path)`.
3. Dispatches to the correct operation.
4. Prints results as a table to stdout.

Table format (list/search):
```
ID   KEY                VALUE                      TAGS        CREATED
1    deploy-branch      feature/memory-system       git,deploy  2026-03-26
2    preferred-model    claude-sonnet-4-5           prefs       2026-03-26
```

---

## Skill: `skills/rightmemory/SKILL.md`

**Access pattern: direct `sqlite3` bash commands — no MCP, no custom binary.**

The skill instructs Claude Code to run `sqlite3` shell commands directly against `$HOME/memory.db`. This is the "System Skill Pattern" — a SKILL.md that teaches Claude the mental model + exact SQL to run against a local SQLite file.

Why `sqlite3` not a custom Rust binary:
- `sqlite3` is a standard system tool. Available on macOS by default and on all supported Linux distros (or installable via package manager). Doctor check can warn if absent.
- `$HOME/memory.db` resolves correctly inside the agent session because the shell wrapper sets `export HOME=<agent_path>` before `exec claude`. No hardcoded absolute paths in the skill.
- SKILL.md can embed exact, auditable SQL. Agent behavior is transparent.
- No compilation step, no PATH complexity, no process lifecycle management.

Why not MCP:
- MCP memory server adds out-of-process coordination and ~55K tokens of tool discovery overhead per session.
- Skills load on-demand (~1K baseline tokens). Token-efficient by design.
- Consistent with `rightskills` and `rightcron` — this codebase uses skills for agent-facing functionality, not MCP.
- Single-agent memory is a local file write. No cross-process coordination needed.

Skill YAML frontmatter:
```yaml
---
name: rightmemory
description: >-
  Manages persistent memory for this agent. Store, recall, search, and forget
  facts, decisions, and observations across sessions via a local SQLite database.
  Use when asked to remember something, recall information, search memory, or
  forget stored entries.
version: 0.1.0
---
```

Key operations the SKILL.md specifies:

```bash
# Store
sqlite3 "$HOME/memory.db" \
  "INSERT INTO memories (key,value,tags,source,created_at,updated_at) \
   VALUES ('$KEY','$VALUE','$TAGS','agent',strftime('%s','now'),strftime('%s','now'));"

# Recall by key
sqlite3 "$HOME/memory.db" \
  "SELECT id,key,value,tags,created_at FROM memories \
   WHERE key='$KEY' ORDER BY created_at DESC LIMIT 10;"

# Search (key + value + tags)
sqlite3 "$HOME/memory.db" \
  "SELECT id,key,value,tags FROM memories \
   WHERE key LIKE '%$Q%' OR value LIKE '%$Q%' OR tags LIKE '%$Q%' \
   ORDER BY updated_at DESC LIMIT 20;"

# Forget by id
sqlite3 "$HOME/memory.db" "DELETE FROM memories WHERE id=$ID;"
```

Sandbox note: `sqlite3` must not appear in `excluded_commands` in `settings.json`. The default sandbox config in `codegen/settings.rs` does not exclude it. No changes to `generate_settings()` needed.

---

## Integration into `rightclaw up`

`cmd_up` already runs a per-agent scaffold loop. Add step 10 after `install_builtin_skills`:

```rust
// 10. Initialize per-agent memory DB (v2.3, MEM-01).
// Ensures memory.db exists with current schema before agent starts.
rightclaw::memory::open_db(&agent.path)
    .map_err(|e| miette::miette!("failed to init memory DB for '{}': {e:#}", agent.name))?;
```

Idempotent. `open_db` creates the file if absent, runs migrations only if `user_version` is behind. No-op on every restart after first run.

---

## `install_builtin_skills` Changes

`codegen/skills.rs` adds a third entry to `built_in_skills`:

```rust
const SKILL_RIGHTMEMORY: &str = include_str!("../../../../skills/rightmemory/SKILL.md");

let built_in_skills: &[(&str, &str)] = &[
    ("rightskills/SKILL.md", SKILL_RIGHTSKILLS),
    ("rightcron/SKILL.md", SKILL_RIGHTCRON),
    ("rightmemory/SKILL.md", SKILL_RIGHTMEMORY),  // NEW
];
```

Same always-overwrite semantics as the other built-ins — agents get updated SKILL.md on every `rightclaw up`.

---

## New Cargo Dependencies

Workspace `Cargo.toml`:
```toml
rusqlite = { version = "0.39", features = ["bundled"] }
rusqlite_migration = { version = "2.5" }
```

`crates/rightclaw/Cargo.toml` (core crate only — CLI does not directly touch the DB):
```toml
rusqlite = { workspace = true }
rusqlite_migration = { workspace = true }
```

**Why `rusqlite` not `sqlx`:**
- Project is SQLite-only. No multi-DB portability needed.
- `cmd_memory` is a synchronous CLI operation. Async overhead of sqlx adds nothing.
- `rusqlite` + `sqlx` in the same workspace is a semver hazard (both link libsqlite3-sys — pin-in-lockstep or conflict).
- `rusqlite` 0.39.0 is current (verified 2026-03-26).

**Why `bundled` feature:**
- RightClaw is a system-level tool distributed via `install.sh`. Bundling SQLite avoids dependency on the system SQLite version (which varies across distros and macOS). First compile is slower; incremental builds are unaffected.

**Why `rusqlite_migration` not `refinery`:**
- SQLite-only project — no need for refinery's multi-DB support.
- `user_version`-based tracking is lighter than refinery's dedicated migrations table.
- No macros. Simple API: `Migrations::new(vec![M::up(SQL)])`.
- Version 2.5.0 is current (verified 2026-03-26 via crates.io API).

---

## New vs. Modified Components

| Component | Status | What Changes |
|-----------|--------|--------------|
| `crates/rightclaw/src/memory/mod.rs` | NEW | Module entry, `open_db()` |
| `crates/rightclaw/src/memory/migrations.rs` | NEW | Schema SQL, `Migrations` instance |
| `crates/rightclaw/src/memory/store.rs` | NEW | `MemoryStore`, `MemoryRow` |
| `crates/rightclaw/src/memory/error.rs` | NEW | `MemoryError` (thiserror) |
| `crates/rightclaw/src/lib.rs` | MODIFIED | Add `pub mod memory;` |
| `crates/rightclaw/src/codegen/skills.rs` | MODIFIED | Add `rightmemory` to built-ins array |
| `crates/rightclaw-cli/src/main.rs` | MODIFIED | Add `Commands::Memory` + `cmd_memory()` |
| `skills/rightmemory/SKILL.md` | NEW | Embedded skill, `sqlite3` bash instructions |
| `Cargo.toml` (workspace) | MODIFIED | Add `rusqlite` + `rusqlite_migration` |
| `crates/rightclaw/Cargo.toml` | MODIFIED | Add deps from workspace |

No new crates. No new templates (skill is inline SKILL.md, not a minijinja template).

---

## Build Order

Dependencies flow in one direction — no cycles:

1. `skills/rightmemory/SKILL.md` — must exist first (needed by `include_str!` in skills.rs at compile time)
2. `Cargo.toml` workspace + core crate — add rusqlite deps
3. `memory/error.rs` — MemoryError type (no deps within the module)
4. `memory/migrations.rs` — schema SQL constants, Migrations instance
5. `memory/store.rs` — MemoryStore (depends on error.rs + migrations.rs)
6. `memory/mod.rs` — re-exports + open_db()
7. `lib.rs` — add `pub mod memory;`
8. `codegen/skills.rs` — add SKILL_RIGHTMEMORY entry (depends on SKILL.md existing on disk)
9. `cmd_up` in CLI — add `open_db` call in scaffold loop (depends on memory module via lib.rs)
10. `Commands::Memory` + `cmd_memory` in CLI — depends on memory module

Each step is independently compilable. Tests can be written per-step.

---

## Data Flows

### Agent stores a memory (via skill at runtime)

```
User says "remember X"
  → CC sees rightmemory skill in SKILL.md metadata (on-demand load)
  → CC reads SKILL.md full content
  → CC runs: sqlite3 "$HOME/memory.db" "INSERT INTO memories ..."
  → SQLite writes to ~/.rightclaw/agents/<name>/memory.db
  → CC reports: "Stored as memory #42"
```

### User inspects memory via CLI

```
rightclaw memory search "deployment"
  → cmd_memory() resolves ~/.rightclaw/agents/<name>/memory.db
  → MemoryStore::search("deployment")
  → rusqlite: SELECT WHERE key LIKE '%deployment%' OR value LIKE '%deployment%' ...
  → table printed to stdout
```

### `rightclaw up` initializes DB

```
rightclaw up
  → per-agent scaffold loop (step 10)
  → open_db(&agent.path)
  → Connection::open("memory.db") + WAL pragma + migrations.to_latest()
  → memory.db exists with correct schema
  → process-compose starts agent
```

### `rightclaw memory vacuum`

```
rightclaw memory vacuum --agent right
  → MemoryStore::open()
  → conn.execute_batch("VACUUM;")
  → println!("Vacuumed memory.db for agent 'right'")
```

---

## Anti-Patterns

### Anti-Pattern 1: Shared memory.db across agents

**What people do:** Place `memory.db` in `~/.rightclaw/` (global home) for convenience.
**Why it's wrong:** Violates per-agent isolation. Agents write contradictory facts to the same store. Out of scope per PROJECT.md ("Shared memory between agents (future — MCP memory server)").
**Do this instead:** `~/.rightclaw/agents/<name>/memory.db` — strictly per-agent.

### Anti-Pattern 2: MCP server for memory

**What people do:** Run a local MCP memory server alongside each agent process.
**Why it's wrong:** Adds process lifecycle management, ~55K token overhead per session for tool discovery, out-of-process coordination for what is a local file write.
**Do this instead:** SKILL.md with `sqlite3` bash commands. Zero overhead, transparent SQL, consistent with existing skill patterns.

### Anti-Pattern 3: Delete and recreate memory.db on `rightclaw up`

**What people do:** Regenerate memory.db on each `up` cycle to ensure clean state (following the settings.json regeneration pattern).
**Why it's wrong:** Destroys persistent memory on every restart. Settings.json is configuration (safe to regenerate). memory.db is accumulated agent knowledge (must survive restarts).
**Do this instead:** `open_db` is idempotent. Migrations are no-ops when schema is current. Never delete the DB file.

### Anti-Pattern 4: Putting memory.db inside `.claude/`

**What people do:** Store at `.claude/memory.db` to co-locate with other CC files.
**Why it's wrong:** `.claude/settings.json` is regenerated on every `up`. Future scaffold changes to `.claude/` contents could interfere with the DB. The directory is CC-internal territory.
**Do this instead:** Flat in agent root: `~/.rightclaw/agents/<name>/memory.db`.

### Anti-Pattern 5: Async MemoryStore

**What people do:** Wrap `rusqlite` in `tokio::spawn_blocking` for consistency with the async main.
**Why it's wrong:** Unnecessary complexity. SQLite is inherently single-writer/synchronous. `cmd_memory` is a CLI query — latency is irrelevant. All existing non-process-compose commands (`cmd_list`, `cmd_doctor`) are sync fns.
**Do this instead:** Synchronous `MemoryStore`. `cmd_memory` is a sync fn. `open_db()` called from `cmd_up`'s scaffold loop is sync (the `async fn cmd_up` awaits only the process-compose spawn, not the scaffold steps).

---

## Integration Points

### External

| Integration | Pattern | Notes |
|-------------|---------|-------|
| `sqlite3` binary (agent sandbox) | SKILL.md bash calls | Must not be in sandbox `excluded_commands`; standard system tool |
| `rusqlite` (CLI) | Rust library, bundled SQLite | No system sqlite3 dep for CLI; independent of agent-facing sqlite3 |

### Internal

| Boundary | Communication | Notes |
|----------|--------------|-------|
| `memory::open_db` ↔ `cmd_up` scaffold loop | Direct sync call | Step 10 in per-agent loop |
| `memory::MemoryStore` ↔ `cmd_memory` | Instantiate + call methods | CLI owns formatting |
| `codegen::install_builtin_skills` ↔ `skills/rightmemory/SKILL.md` | `include_str!` at compile time | Same pattern as rightskills/rightcron |

---

## Sources

- Direct codebase inspection: `/home/wb/dev/rightclaw/crates/` (2026-03-26)
- [rusqlite 0.39.0 on crates.io](https://crates.io/crates/rusqlite) — version verified 2026-03-26
- [rusqlite_migration 2.5.0 on crates.io](https://crates.io/crates/rusqlite_migration) — version verified 2026-03-26
- [Rust ORMs in 2026: rusqlite vs sqlx](https://aarambhdevhub.medium.com/rust-orms-in-2026-diesel-vs-sqlx-vs-seaorm-vs-rusqlite-which-one-should-you-actually-use-706d0fe912f3) — when to use which
- [sqlx + rusqlite semver hazard](https://github.com/launchbadge/sqlx/discussions/3295) — why not to mix them
- [The System Skill Pattern](https://www.shruggingface.com/blog/the-system-skill-pattern) — SKILL.md + sqlite3 bash approach
- [Claude Skills vs MCP](https://intuitionlabs.ai/articles/claude-skills-vs-mcp) — token overhead, layer distinction

---
*Architecture research for: RightClaw v2.3 Memory System*
*Researched: 2026-03-26*
