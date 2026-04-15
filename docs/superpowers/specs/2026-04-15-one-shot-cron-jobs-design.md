# One-Shot Cron Jobs

## Problem

The cron system only supports recurring jobs with 5-field cron expressions. Agents cannot schedule a task to run once at a specific time ("remind me at 3pm", "run this in 30 minutes"). They must create a recurring spec and manually delete it after one execution — error-prone and wasteful.

## Design Decisions

- **`run_at` field (ISO8601)** as alternative to `schedule` for absolute-time one-shot jobs
- **`recurring` bool** for cron-expression one-shot jobs (fire once at next match, auto-delete)
- **Reconcile-based firing** for `run_at` — check `run_at <= now` on each 5-second tick, no sleeping tokio tasks
- **Always fire overdue** — if bot was down when `run_at` passed, fire on first reconcile after restart
- **Partial update** — `cron_update` only modifies fields that are passed

## Changes

### 1. Migration (v13)

Add two columns to `cron_specs`:

```sql
ALTER TABLE cron_specs ADD COLUMN recurring INTEGER NOT NULL DEFAULT 1;
ALTER TABLE cron_specs ADD COLUMN run_at TEXT;
```

`schedule` remains `TEXT NOT NULL` from v6. For `run_at` specs, `schedule` stores empty string `""`. Validation enforced in Rust — exactly one of `schedule`/`run_at` must be meaningful.

Migration is idempotent: use `pragma_table_info` check before each ALTER.

### 2. Rust types (`cron_spec.rs`)

New enum for schedule representation:

```rust
pub enum ScheduleKind {
    Recurring(String),           // 5-field cron, fires repeatedly
    OneShotCron(String),         // 5-field cron, fires once then auto-deletes
    RunAt(DateTime<Utc>),        // absolute time, fires once then auto-deletes
}
```

`CronSpec` replaces `schedule: String` with `schedule_kind: ScheduleKind`. DB mapping:

| `recurring` | `run_at`  | `schedule` | `ScheduleKind`      |
|-------------|-----------|------------|----------------------|
| `1`         | `NULL`    | cron expr  | `Recurring(expr)`    |
| `0`         | `NULL`    | cron expr  | `OneShotCron(expr)`  |
| any         | ISO8601   | `""`       | `RunAt(datetime)`    |

`PartialEq` excludes `triggered_at` (unchanged from current behavior).

### 3. MCP tool parameters

**`cron_create`** — new optional fields:

```rust
pub struct CronCreateParams {
    pub job_name: String,
    pub schedule: Option<String>,       // 5-field cron; required if no run_at
    pub prompt: String,
    pub recurring: Option<bool>,        // default true; ignored if run_at set
    pub run_at: Option<String>,         // ISO8601; mutually exclusive with schedule
    pub lock_ttl: Option<String>,
    pub max_budget_usd: Option<f64>,
}
```

Validation:
- Exactly one of `schedule`/`run_at` must be provided
- `run_at` + `schedule` both set → error
- Neither set → error
- `run_at` parses as `DateTime<Utc>`; past values allowed
- `run_at` implies `recurring: false` regardless of what's passed

**`cron_update`** — partial update (all fields optional except `job_name`):

```rust
pub struct CronUpdateParams {
    pub job_name: String,               // identifies the spec
    pub schedule: Option<String>,       // sets schedule, clears run_at
    pub run_at: Option<String>,         // sets run_at, clears schedule, forces recurring=false
    pub prompt: Option<String>,
    pub recurring: Option<bool>,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: Option<f64>,
}
```

Semantics:
- Only passed fields are updated; others keep current values
- `schedule` passed → clears `run_at`, sets `schedule`
- `run_at` passed → clears `schedule` (set to `""`), forces `recurring=false`
- Both `schedule` + `run_at` passed → error
- No fields besides `job_name` → error

**`cron_list`** response — add `recurring` and `run_at` fields to each entry.

Other tools (`cron_delete`, `cron_trigger`, `cron_list_runs`, `cron_show_run`) unchanged.

### 4. Reconcile loop (`cron.rs`)

Add a `run_at` processing step at the beginning of each `reconcile_jobs()` tick, before the existing recurring logic:

```
fn reconcile_jobs():
    // NEW: fire overdue run_at specs
    overdue = SELECT * FROM cron_specs WHERE run_at IS NOT NULL AND run_at <= now()
    for spec in overdue:
        if is_locked(spec):
            continue  // try next tick
        spawn execute_job(spec)
        on completion: delete_spec(spec.job_name)

    // EXISTING: reconcile recurring and one-shot-cron specs
    ...
```

`run_at` specs never get in-memory handles — no `run_job_loop()`, no sleeping tasks. Fire-and-forget from reconcile.

**One-shot cron specs** (`OneShotCron`): go through normal `run_job_loop()`. After `execute_job()` completes, check `recurring` flag — if false, call `delete_spec()` and break out of loop.

### 5. Skill update (`skills/rightcron/SKILL.md`)

Update `cron_create` docs with new parameters and examples:

```
# Recurring (default)
cron_create(job_name="health-check", schedule="7 * * * *", prompt="...")

# One-shot cron (fire once at next match, then auto-delete)
cron_create(job_name="deploy-check", schedule="30 15 * * *", recurring=false, prompt="...")

# Run at specific time (fire once, then auto-delete)
cron_create(job_name="remind-deploy", run_at="2026-04-15T15:30:00Z", prompt="...")
```

Update `cron_update` docs — partial update semantics with examples:

```
# Change only the prompt
cron_update(job_name="health-check", prompt="new prompt here")

# Change only the schedule
cron_update(job_name="health-check", schedule="17 */2 * * *")

# Switch from recurring to one-shot run_at
cron_update(job_name="health-check", run_at="2026-04-16T10:00:00Z")
```

Update `cron_list` docs to mention new fields in response.

## Testing

### Unit tests (`cron_spec.rs`)

1. `create_spec` with `run_at`, no `schedule` — succeeds, stored as `RunAt`
2. `create_spec` with both `schedule` + `run_at` — error
3. `create_spec` with neither — error
4. `create_spec` with invalid ISO8601 `run_at` — error
5. `create_spec` with past `run_at` — succeeds
6. `create_spec` with `recurring: false` + `schedule` — stored as `OneShotCron`
7. `load_specs_from_db` — all three `ScheduleKind` variants round-trip correctly
8. `update_spec` partial — change only `prompt`, verify other fields unchanged
9. `update_spec` partial — set `schedule`, verify `run_at` cleared
10. `update_spec` partial — set `run_at`, verify `schedule` cleared and `recurring` forced false
11. `update_spec` with both `schedule` + `run_at` — error
12. `update_spec` with no fields — error

### Integration tests (`cron.rs`)

13. `run_at` in past → reconcile fires job, spec deleted after completion
14. `run_at` in future → reconcile skips, spec remains
15. `OneShotCron` → fires once via `run_job_loop`, spec deleted after completion
16. `Recurring` → fires, spec NOT deleted
17. `run_at` with active lock → reconcile skips, spec remains, fires on next tick after lock expires

### Migration test

18. v13 migration idempotent — existing specs get `recurring=1, run_at=NULL`, re-running migration is safe

## Files Modified

| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/sql/v13_one_shot_cron.sql` | New migration |
| `crates/rightclaw/src/memory/migrations.rs` | Register v13 |
| `crates/rightclaw/src/cron_spec.rs` | `ScheduleKind` enum, partial update, new validation |
| `crates/bot/src/cron.rs` | `run_at` processing in reconcile, one-shot delete after fire |
| `crates/rightclaw-cli/src/right_backend.rs` | Updated `CronCreateParams`, `CronUpdateParams` |
| `crates/rightclaw-cli/src/memory_server.rs` | Updated param structs |
| `crates/rightclaw-cli/src/aggregator.rs` | Updated tool schemas + `with_instructions()` |
| `skills/rightcron/SKILL.md` | New parameters, examples, partial update docs |
