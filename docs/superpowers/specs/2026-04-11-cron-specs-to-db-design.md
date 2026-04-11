# Cron Specs to DB

## Problem

Cron specs are stored as YAML files in `crons/`. Agents in sandbox write to `/sandbox/crons/` but the bot reads `agent_dir/crons/` on the host — different filesystems. Reverse sync does not cover cron YAML. Two storage systems (YAML specs + SQLite results) split the source of truth.

## Solution

Move cron specs into SQLite (`memory.db`). New `cron_specs` table. 4 MCP tools for CRUD. `load_specs()` reads from DB instead of filesystem. No more file-based polling for spec changes.

## DB Schema — V6 Migration

```sql
CREATE TABLE cron_specs (
    job_name       TEXT PRIMARY KEY,
    schedule       TEXT NOT NULL,
    prompt         TEXT NOT NULL,
    lock_ttl       TEXT,                          -- "30m", "1h", etc. NULL = default 30m
    max_budget_usd REAL NOT NULL DEFAULT 1.0,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
```

Existing YAML files in `crons/` are ignored after migration — `load_specs()` no longer reads them.

## MCP Tools (4 new)

### `cron_create`

Create a new cron spec.

Parameters:
- `job_name` (string, required) — must match `^[a-z0-9][a-z0-9-]*$`
- `schedule` (string, required) — 5-field cron expression, validated by parsing through `cron::Schedule`
- `prompt` (string, required) — non-empty
- `lock_ttl` (string, optional) — e.g. "30m", "1h". Default 30m.
- `max_budget_usd` (number, optional) — default 1.0, must be > 0

Behavior:
- Validates all inputs
- Returns warning if schedule uses round minutes (:00 or :30)
- Errors if `job_name` already exists
- Inserts row with `created_at = updated_at = NOW()`

### `cron_update`

Update an existing cron spec. Full replacement, not partial.

Parameters: same as `cron_create`.

Behavior:
- Validates all inputs
- Errors if `job_name` does not exist
- Updates all fields, sets `updated_at = NOW()`

### `cron_delete`

Delete a cron spec.

Parameters:
- `job_name` (string, required)

Behavior:
- Errors if `job_name` does not exist
- Deletes the row
- Also removes lock file (`crons/.locks/<job_name>.json`) if present

### `cron_list`

List all current cron specs.

Parameters: none.

Returns: all specs with `job_name`, `schedule`, `prompt`, `lock_ttl`, `max_budget_usd`.

## Cron Engine Changes

### `load_specs()` → `load_specs_from_db()`

Replace filesystem scan with DB query:

```rust
pub fn load_specs_from_db(conn: &rusqlite::Connection) -> HashMap<String, CronSpec> {
    // SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd FROM cron_specs
    // Build HashMap<String, CronSpec> from rows
    // Warn on round minutes (existing behavior)
}
```

Returns the same `HashMap<String, CronSpec>` — downstream reconcile logic unchanged.

### Reconcile loop

No logic change. Calls `load_specs_from_db(&conn)` instead of `load_specs(agent_dir)`. Diff by HashMap keys as before. 60-second poll interval stays.

### Lock files

Remain filesystem-based in `crons/.locks/`. They are ephemeral runtime state, not specs.

## Skill (rightcron/SKILL.md)

Rewrite CRUD section:
- Create: `cron_create(job_name="...", schedule="...", prompt="...")`
- Edit: `cron_update(job_name="...", schedule="...", prompt="...")`
- Delete: `cron_delete(job_name="...")`
- List specs: `cron_list()`
- Run history: `cron_list_runs()` / `cron_show_run()` — unchanged

Remove all references to writing/editing/deleting YAML files.

## What Does NOT Change

- `cron_runs` table and delivery loop
- `cron_list_runs` / `cron_show_run` MCP tools
- Lock mechanism (filesystem, ephemeral)
- `execute_job()` logic
- `CronSpec` struct fields (deserialized from DB row instead of YAML)

## Files to Modify

| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/sql/v6_cron_specs.sql` | New migration |
| `crates/rightclaw/src/memory/migrations.rs` | Wire V6 |
| `crates/bot/src/cron.rs` | `load_specs()` → DB query, remove walkdir/serde_saphyr deps |
| `crates/rightclaw-cli/src/memory_server.rs` | +4 MCP tools (cron_create/update/delete/list) |
| `crates/rightclaw-cli/src/memory_server_http.rs` | +4 MCP tools (HTTP transport) |
| `skills/rightcron/SKILL.md` | Rewrite to use MCP tools |
