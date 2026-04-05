# Phase 27: Cron Runtime — Research

**Researched:** 2026-04-01
**Domain:** Tokio cron task inside `rightclaw bot` — cron expression scheduling, lock-file deduplication, DB migration, MCP server extension
**Confidence:** HIGH

## Summary

Phase 27 adds a cron runtime to `crates/bot/src/cron.rs` as a `tokio::spawn` task running alongside the Telegram dispatcher. The runtime polls `crons/*.yaml` every 60 seconds, parses standard 5-field cron expressions (via `cron 0.16` with a "0 " prefix wrapper), checks lock files for deduplication, and fires `claude -p --agent <name>` subprocesses on schedule.

The bot entry point (`lib.rs`) currently ends with `telegram::run_telegram(...)` and returns. After this phase, it must spawn the cron task before calling into the Telegram dispatcher. The `invoke_cc` pattern in `telegram/worker.rs` is directly reusable — the cron case is simpler: no session management, no JSON schema, no reply parsing.

Memory infrastructure follows an established pattern: add a `.sql` file to `crates/rightclaw/src/memory/sql/`, reference it in `migrations.rs` as `V3_SCHEMA`, and append `M::up(V3_SCHEMA)` to the `MIGRATIONS` lazy. The MCP server (`memory_server.rs`) uses `rmcp`'s `#[tool]` derive — add two new parameter structs and two `#[tool]` methods to `MemoryServer`. Server rename requires `ServerInfo::with_server_info(Implementation { name: "rightclaw".into(), version: ..., ..Default::default() })`.

**Primary recommendation:** Use `cron 0.16` (parse-only library, no async overhead) with a manual `tokio::time::sleep_until` scheduling loop in `cron.rs`. This matches CRON-03 exactly and keeps full control over idempotency (CRON-06).

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01: CC invocation style**
Cron jobs use `--agent <name>` — same as bot dispatch (AGDEF-02). Full agent def inherited: IDENTITY.md + SOUL.md + tool whitelist from sandbox config.

Not used: `--output-format json`, `--json-schema` (cron jobs don't return structured replies — they act autonomously and communicate via `reply` MCP tool).

Command shape:
```
claude -p --agent <name> -- "<job prompt wrapped with lock guard>"
```
Same `HOME` and `cwd` as bot dispatch (`agent_dir`).

**D-02: Subprocess failure routing**
`tracing::error!` only. No Telegram forwarding.

**D-03: Missed runs on restart — Skip**
No catch-up. Next run after startup is the next scheduled time.

**D-04: Cron run history**
- `cron_runs` table in `memory.db` (V3 migration)
- Log files at `agent_dir/crons/logs/<job_name>-<run_id>.txt`
- Insert `status='running'` at start; UPDATE on completion.
- Schema:
```sql
CREATE TABLE cron_runs (
    id          TEXT PRIMARY KEY,
    job_name    TEXT NOT NULL,
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    exit_code   INTEGER,
    status      TEXT NOT NULL,
    log_path    TEXT NOT NULL
);
```

**D-05: MCP server rename + extend**
- Server name: `rightclaw` (was Cargo default from `CARGO_PKG_NAME`)
- Add to `MemoryServer`: `cron_list_runs(job_name?: str, limit?: int)` and `cron_show_run(run_id: str)`
- No `cron_read_log` tool — agent reads log file via `log_path` directly

**D-06: Skill updates deferred to Phase 28**

### Claude's Discretion

None specified.

### Deferred Ideas (OUT OF SCOPE)

- File-watch hot-reload (notify-debouncer-full) — v3.1
- `catch_up: true` YAML field
- Skill update for cron run querying — Phase 28
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CRON-01 | Cron runtime as tokio task inside `rightclaw bot`, alongside Telegram dispatcher | Bot entry point pattern — spawn before `run_telegram`, pass `agent_dir` + `agent_name` |
| CRON-02 | Read `agent_dir/crons/*.yaml` on startup and every 60s | `walkdir` already in crate deps; `tokio::time::interval(Duration::from_secs(60))` |
| CRON-03 | Schedule parsed via `cron 0.16` → `chrono::DateTime<Utc>` for next-run | `cron` 0.16 uses 7-field; wrap YAML 5-field with `"0 " + expr + " *"` |
| CRON-04 | Lock file check before executing each job | Stat `crons/.locks/<name>.json`, parse `heartbeat`, compare to `Utc::now()` minus `lock_ttl` |
| CRON-05 | Job executed as `claude -p --agent <name> -- "<prompt>"` subprocess | Reuse `invoke_cc` pattern verbatim minus session/schema/JSON-output logic |
| CRON-06 | Reconciler idempotent — re-reading cron files doesn't duplicate jobs | Map keyed by job name; replace entry on reload; sleep loop wakes at next-run |
</phase_requirements>

---

## Cron YAML Spec

Source: `skills/cronsync/SKILL.md` (read directly), confidence HIGH.

### Fields

| Field | Type | Required | Default | Notes |
|-------|------|----------|---------|-------|
| `schedule` | string | Yes | — | Standard 5-field Unix cron (`min hour dom mon dow`) |
| `prompt` | string | Yes | — | Raw task text; cron.rs wraps it with lock guard logic |
| `lock_ttl` | string | No | `30m` | Duration before lock considered stale. Format: `10m`, `1h`, `30m` |
| `max_turns` | integer | No | — | Passed as `--max-turns` to claude -p |

### Example files

```yaml
# crons/deploy-check.yaml
schedule: "*/5 * * * *"
lock_ttl: 10m
max_turns: 5
prompt: "Check CI status for all open PRs, post comment if broken"
```

```yaml
# crons/morning-briefing.yaml
schedule: "0 9 * * 1-5"
lock_ttl: 30m
prompt: "Gather open PRs, failing tests, pending reviews. Post summary."
```

### YAML deserialization struct

```rust
#[derive(Debug, serde::Deserialize)]
pub struct CronSpec {
    pub schedule: String,
    pub prompt: String,
    pub lock_ttl: Option<String>,    // default "30m" if None
    pub max_turns: Option<u32>,
}
```

Use `serde-saphyr` (already in workspace) to parse — consistent with rest of codebase.

### Lock file format

Path: `crons/.locks/<name>.json`

```json
{"heartbeat": "2026-03-22T10:05:00Z"}
```

UTC ISO 8601 with `Z` suffix. Written at subprocess start, deleted on completion (success or failure).

---

## Bot Entry Point — Spawning Pattern

Source: `crates/bot/src/lib.rs` (read directly), confidence HIGH.

`run_async` currently resolves agent dir, opens memory.db, resolves token, calls `deleteWebhook`, then calls `telegram::run_telegram(token, allowed_chat_ids, agent_dir).await`. This last call runs forever (dispatcher loop).

**Insertion point:** Spawn the cron task between `deleteWebhook` and `telegram::run_telegram`. Pass `agent_dir.clone()` and `args.agent.clone()` to the cron task.

```rust
// After deleteWebhook, before run_telegram:
let cron_agent_dir = agent_dir.clone();
let cron_agent_name = args.agent.clone();
tokio::spawn(async move {
    cron::run_cron_task(cron_agent_dir, cron_agent_name).await;
});

telegram::run_telegram(token, config.allowed_chat_ids, agent_dir).await
```

The cron task runs indefinitely in the background. When the bot process exits (SIGTERM), all spawned tokio tasks are dropped automatically — the cron task does not need explicit shutdown wiring because cron subprocess children already use `kill_on_drop(true)`.

**Key detail:** `args.agent` is already the agent name string. No need to derive it from `agent_dir.file_name()` — use `args.agent` directly.

---

## invoke_cc Pattern — Cron Adaptation

Source: `crates/bot/src/telegram/worker.rs` (read directly), confidence HIGH.

The full `invoke_cc` in `worker.rs` does: binary resolution → DB open → session lookup/create → command build → spawn → wait_with_output → parse JSON output.

For cron, the relevant subset is:

```rust
// Binary resolution (identical)
let cc_bin = which::which("claude")
    .or_else(|_| which::which("claude-bun"))
    .map_err(|_| CronError::BinaryNotFound)?;

// Command build (simplified — no session, no JSON schema)
let mut cmd = tokio::process::Command::new(&cc_bin);
cmd.arg("-p");
cmd.arg("--agent").arg(agent_name);     // always present (D-01: no resume in cron)
if let Some(max_turns) = spec.max_turns {
    cmd.arg("--max-turns").arg(max_turns.to_string());
}
cmd.arg("--").arg(&wrapped_prompt);
cmd.env("HOME", agent_dir);
cmd.current_dir(agent_dir);
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::piped());
cmd.stderr(Stdio::piped());
cmd.kill_on_drop(true);                  // killed on bot SIGTERM (BOT-04 parity)
```

**What is NOT needed for cron:**
- Session lookup/create (no continuity between cron runs)
- `--output-format json` (D-01 explicitly excludes it)
- `--json-schema` (no structured reply)
- Reply output parsing (D-02: log only)

**stdout/stderr capture strategy (D-04):** Instead of `format_error_reply`, pipe output to a log file at `crons/logs/<job_name>-<run_id>.txt`. Since `wait_with_output()` collects all output in memory first, use `wait_with_output()` then write to disk — avoids interleaved writes. The log directory must be created before the first job runs.

---

## Cron Scheduling Approach

Source: crates.io search + docs.rs for `cron` 0.16, confidence HIGH.

### Recommendation: `cron 0.16` (parse-only) + manual `tokio::time::sleep_until` loop

CRON-03 mandates `cron 0.16` explicitly. This is the right choice regardless — it is a lightweight, pure parser that returns `chrono::DateTime<Utc>` iterators. No runtime, no threads, no background executors.

**Critical gotcha:** `cron 0.16` requires **7-field** expressions: `sec min hour dom mon dow year`. The SKILL.md and all existing user cron specs use standard **5-field** Unix format (`min hour dom mon dow`). Conversion: prepend `"0 "` (fixed seconds=0) and append `" *"` (any year).

```rust
/// Convert 5-field user expression to 7-field cron crate format.
fn to_7field(expr: &str) -> String {
    format!("0 {} *", expr.trim())
}

// "*/5 * * * *"  →  "0 */5 * * * * *"
// "0 9 * * 1-5"  →  "0 0 9 * * 1-5 *"
```

**Scheduling loop pattern:**

```rust
use cron::Schedule;
use std::str::FromStr;

let schedule = Schedule::from_str(&to_7field(&spec.schedule))?;

loop {
    let now = chrono::Utc::now();
    let next = schedule.after(&now).next();     // Option<DateTime<Utc>>
    let Some(fire_at) = next else {
        tracing::warn!(job = %name, "schedule has no future fires — skipping");
        break;
    };
    let delay = (fire_at - now).to_std().unwrap_or(Duration::ZERO);
    tokio::time::sleep(delay).await;

    // Re-check: if bot was paused/overloaded, actual time may have drifted past fire_at.
    // Proceed to execute (no skip on drift — next schedule computation handles it).
    execute_job(&name, &spec, &agent_dir, &agent_name).await;
}
```

### Why not `tokio-cron-scheduler` (0.15)?

- Adds its own background executor, job registry, and storage layer — heavyweight for what amounts to a single-file task loop.
- CRON-06 idempotency is harder: the scheduler's internal job registry can duplicate jobs on config reload.
- Dependency not in workspace — would need to add it.
- CRON-03 explicitly names `cron 0.16`, not `tokio-cron-scheduler`.

### Why not polling every second?

Current CRON-03 design uses next-run sleep. A 60-second polling fallback (CRON-02) handles the YAML hot-reload requirement — it re-reads specs and resets the per-job sleep timers, not firing events.

### Idempotency (CRON-06)

Use a `HashMap<String, JoinHandle<_>>` keyed by job name. On each 60-second reload:
1. Read all `crons/*.yaml` into a new map.
2. For jobs no longer in the spec (deleted): abort the old `JoinHandle`.
3. For jobs with changed `schedule` or `prompt`: abort old handle, spawn new.
4. For unchanged jobs: leave running handle untouched.
5. Insert new jobs for specs not yet in the handle map.

This requires serializing the spec hash for change detection — compute `sha256(schedule + prompt + lock_ttl + max_turns)` using `sha2` or just concatenate fields into a stable string for comparison (simpler, no extra dep).

**Simpler approach (adequate for Phase 27):** Since `cron.rs` owns the spec state in a `HashMap<String, CronSpec>`, compare fields directly. No hash needed.

---

## Lock File Implementation

Source: `skills/cronsync/SKILL.md` (read directly), confidence HIGH.

### Lock TTL parsing

`lock_ttl` field is a duration string like `"30m"`, `"10m"`, `"1h"`. No standard Rust crate in workspace handles this. Options:

1. **Hand-roll a minimal parser** — parse trailing `m`/`h` suffix, multiply by 60 or 3600.
   Simple, zero deps, <10 lines.
2. **Add `humantime` crate** — `humantime::parse_duration("30m")` returns `std::time::Duration`.
   But `humantime` format is `"30min"` not `"30m"` — would need to adapt or reject the SKILL.md format.

**Recommendation:** Hand-roll `parse_lock_ttl(s: &str) -> Result<chrono::Duration>` supporting `m` (minutes) and `h` (hours) suffixes. This matches SKILL.md format exactly and adds zero dependencies.

```rust
fn parse_lock_ttl(s: &str) -> Result<chrono::Duration, CronError> {
    if let Some(mins) = s.strip_suffix('m') {
        let n: i64 = mins.trim().parse()?;
        return Ok(chrono::Duration::minutes(n));
    }
    if let Some(hrs) = s.strip_suffix('h') {
        let n: i64 = hrs.trim().parse()?;
        return Ok(chrono::Duration::hours(n));
    }
    Err(CronError::InvalidLockTtl(s.to_string()))
}
```

### Lock check algorithm

Before executing a job:

```rust
let lock_path = agent_dir.join("crons/.locks").join(format!("{name}.json"));

if lock_path.exists() {
    let raw = std::fs::read_to_string(&lock_path)?;
    let lock: LockFile = serde_json::from_str(&raw)?;
    let heartbeat = lock.heartbeat;  // chrono::DateTime<Utc>
    let ttl = parse_lock_ttl(spec.lock_ttl.as_deref().unwrap_or("30m"))?;
    if Utc::now() - heartbeat < ttl {
        tracing::info!(job = %name, "skipping — previous run still active (lock fresh)");
        return Ok(());
    }
    // Stale lock — delete and continue
    std::fs::remove_file(&lock_path).ok();
}

// Write lock
std::fs::create_dir_all(lock_path.parent().unwrap())?;
let lock_json = serde_json::json!({"heartbeat": Utc::now().to_rfc3339()});
std::fs::write(&lock_path, lock_json.to_string())?;

// ... execute subprocess ...

// Delete lock on completion (success or failure)
std::fs::remove_file(&lock_path).ok();
```

**Note:** Lock file write and delete are sync filesystem ops — acceptable here because they are instantaneous on local disk. No async needed.

---

## Migration Infrastructure — V3 Approach

Source: `crates/rightclaw/src/memory/migrations.rs` + `sql/` files (read directly), confidence HIGH.

### Current state

- `migrations.rs` declares `MIGRATIONS` as `LazyLock<Migrations<'static>>` with `[M::up(V1_SCHEMA), M::up(V2_SCHEMA)]`.
- Each schema is an `include_str!("sql/vN_xxx.sql")` constant.
- `user_version` after V2 is `2` (verified by test in `mod.rs`).

### V3 addition pattern

1. Create `crates/rightclaw/src/memory/sql/v3_cron_runs.sql`:
```sql
-- V3 schema: cron_runs
-- Source: Phase 27 decision D-04

CREATE TABLE IF NOT EXISTS cron_runs (
    id          TEXT    PRIMARY KEY,          -- UUID
    job_name    TEXT    NOT NULL,
    started_at  TEXT    NOT NULL,             -- ISO8601 UTC
    finished_at TEXT,                         -- NULL while running
    exit_code   INTEGER,                      -- NULL while running
    status      TEXT    NOT NULL,             -- 'running' | 'success' | 'failed'
    log_path    TEXT    NOT NULL              -- absolute path to log file
);
```

2. In `migrations.rs`:
```rust
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![
        M::up(V1_SCHEMA),
        M::up(V2_SCHEMA),
        M::up(V3_SCHEMA),
    ]));
```

3. Update the `user_version_is_2` test in `memory/mod.rs` to assert `user_version == 3`.

**Write pattern for cron_runs:** The bot crate opens its own `rusqlite::Connection` per `invoke_cc` call (same as worker.rs). Cron runner follows the same: open a connection to `agent_dir/memory.db` for each job run using `rightclaw::memory::open_connection`. Insert at start, UPDATE on completion.

```rust
// Insert at job start
conn.execute(
    "INSERT INTO cron_runs (id, job_name, started_at, status, log_path)
     VALUES (?1, ?2, ?3, 'running', ?4)",
    rusqlite::params![run_id, job_name, started_at, log_path],
)?;

// UPDATE on completion
conn.execute(
    "UPDATE cron_runs SET finished_at=?1, exit_code=?2, status=?3 WHERE id=?4",
    rusqlite::params![finished_at, exit_code, status_str, run_id],
)?;
```

---

## MCP Server — Current Tool Registration Pattern

Source: `crates/rightclaw-cli/src/memory_server.rs` (read directly), confidence HIGH.

### Pattern for adding a tool

1. Add a parameter struct with `#[derive(Debug, Deserialize, JsonSchema)]`.
2. Add an `async fn` method to `MemoryServer` impl block with `#[tool(description = "...")]`.
3. The method signature: `async fn name(&self, Parameters(params): Parameters<MyParams>) -> Result<CallToolResult, McpError>`.
4. Lock `self.conn`, call a store function, return `CallToolResult::success(vec![Content::text(...)])`.

### Server rename (D-05)

Current `get_info()`:
```rust
fn get_info(&self) -> ServerInfo {
    ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        .with_instructions("RightClaw memory tools: store, recall, search, forget")
}
```

`ServerInfo::new` creates a default where `server_info` is populated from `CARGO_PKG_NAME` / `CARGO_PKG_VERSION` at build time. To override the name to `"rightclaw"`, use `with_server_info`:

```rust
use rmcp::model::Implementation;

fn get_info(&self) -> ServerInfo {
    ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation {
            name: "rightclaw".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        })
        .with_instructions("RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run")
}
```

### New tool parameter structs

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronListRunsParams {
    #[schemars(description = "Filter by job name (optional)")]
    pub job_name: Option<String>,
    #[schemars(description = "Maximum number of runs to return (default: 20)")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CronShowRunParams {
    #[schemars(description = "Run ID (UUID)")]
    pub run_id: String,
}
```

### SQL queries for the new tools

```sql
-- cron_list_runs
SELECT id, job_name, started_at, finished_at, status, exit_code, log_path
FROM cron_runs
WHERE (?1 IS NULL OR job_name = ?1)
ORDER BY started_at DESC
LIMIT ?2;

-- cron_show_run
SELECT id, job_name, started_at, finished_at, status, exit_code, log_path
FROM cron_runs
WHERE id = ?1;
```

`MemoryServer` already has `conn: Arc<Mutex<rusqlite::Connection>>` — the new tools lock and query it the same way as `recall` and `search`. Add a `cron_run_to_json()` helper similar to `entry_to_json()`.

**Shared state note:** `MemoryServer` is the MCP server process launched by CC — it is separate from the bot process. The `cron_runs` table is written by the bot process (via its own connection to `memory.db`) and read by the MCP server. WAL mode (already enabled) allows concurrent readers and a single writer without blocking.

---

## Standard Stack

### Core (already in workspace)

| Library | Workspace Version | Purpose | Role in Phase 27 |
|---------|------------------|---------|-----------------|
| `cron` | 0.16 (to add) | Parse cron expressions | CRON-03: `Schedule::from_str`, `.after().next()` |
| `tokio` | 1.50 | Async runtime | `tokio::spawn`, `tokio::time::sleep`, `tokio::process::Command` |
| `chrono` | 0.4 | DateTime arithmetic | Next-run computation, lock TTL comparison, ISO8601 timestamps |
| `serde-saphyr` | 0.0 | YAML parsing | Parse `crons/*.yaml` into `CronSpec` struct |
| `serde_json` | 1.0 | Lock file JSON | Read/write `{"heartbeat": "..."}` |
| `rusqlite` | 0.39 | cron_runs DB writes | Insert/update run records |
| `uuid` | 1 | Run IDs | `Uuid::new_v4()` per run |
| `walkdir` | 2.5 | Directory scan | Enumerate `crons/*.yaml` |
| `which` | 7.0 | CC binary | Same as worker.rs |
| `tracing` | 0.1 | Job execution logs | D-02: `tracing::error!` on failure |

### New dependency needed

`cron = "0.16"` must be added to `crates/bot/Cargo.toml` and to `[workspace.dependencies]` in root `Cargo.toml`.

```toml
# workspace Cargo.toml
cron = "0.16"

# crates/bot/Cargo.toml
cron = { workspace = true }
```

No other new dependencies needed.

---

## Architecture Patterns

### Recommended Module Structure

```
crates/bot/src/
  lib.rs            -- spawn cron task before telegram::run_telegram
  cron.rs           -- new: CronSpec, CronRunner, run_cron_task
  telegram/
    ...             -- unchanged
```

### cron.rs internal structure

```
cron.rs
  CronSpec           -- deserialized from YAML
  LockFile           -- {"heartbeat": DateTime<Utc>}
  CronRunRecord      -- struct for DB insert/update
  fn run_cron_task(agent_dir, agent_name) -> ! (never returns, loop)
  fn load_specs(agent_dir) -> HashMap<String, CronSpec>
  fn schedule_jobs(specs, agent_dir, agent_name) -- spawns per-job tasks
  fn execute_job(name, spec, agent_dir, agent_name) -> Result<()>
  fn check_lock(name, spec, agent_dir) -> bool (skip if locked)
  fn write_lock / delete_lock
  fn parse_lock_ttl(s: &str) -> Result<chrono::Duration>
```

### Per-job task pattern

The reconciler loop (every 60s in `run_cron_task`) manages `HashMap<String, JoinHandle<()>>`. Each job gets its own `tokio::spawn` loop:

```
run_cron_task (outer loop, 60s poll):
  load_specs()
  diff old_specs vs new_specs
  abort removed/changed handles
  spawn new job loops for new/changed specs

per_job_loop (inner loop, sleeps until next fire):
  compute next_run from schedule
  sleep until next_run
  execute_job()
  loop
```

### Anti-Patterns to Avoid

- **Do not open a persistent DB connection in `run_cron_task`**: `rusqlite::Connection` is `!Send`. Open per-job in `execute_job`, same as `invoke_cc` in worker.rs.
- **Do not use `tokio::time::interval` for per-job scheduling**: Interval drifts — use `sleep_until` computed from `schedule.after(&now).next()` on every iteration.
- **Do not parse 5-field expressions directly with `cron 0.16`**: The crate requires 7 fields. Wrap with `"0 {} *"`.
- **Do not block on log file write**: After `wait_with_output()`, write is synchronous but fast (in-process data to local disk) — acceptable. If the log write fails, log the error and update DB status to `'failed'` anyway.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cron expression parsing | Custom parser | `cron 0.16` | Handles ranges, step values, named months/days |
| Async runtime management | Own thread pool | `tokio::spawn` | Already in use |
| Lock TTL duration string parsing | Full duration parser | 10-line hand-roll for `m`/`h` | Only two units needed; `humantime` uses `min` not `m` |
| Job deduplication | Lockfile-based across processes | HashMap of JoinHandles | Single-process, in-memory is correct |

---

## Common Pitfalls

### Pitfall 1: 5-field vs 7-field cron expression mismatch

**What goes wrong:** `Schedule::from_str("*/5 * * * *")` fails — cron 0.16 requires 7 fields.
**Why it happens:** The SKILL.md and unix convention use 5-field; the cron crate always needs sec + year fields.
**How to avoid:** Always wrap with `format!("0 {} *", user_expr.trim())` before parsing.
**Warning signs:** `FromStr` parse error on startup for every YAML spec.

### Pitfall 2: `rusqlite::Connection` is `!Send`

**What goes wrong:** Trying to store a `Connection` in a struct that crosses `tokio::spawn` boundaries.
**Why it happens:** SQLite connections are not thread-safe.
**How to avoid:** Open a new connection per `execute_job()` call (same pattern as `worker.rs`). Do not store connections in `Arc<Mutex<>>` in the cron module — the bot side is write-only and short-lived.

### Pitfall 3: Clock drift on slow jobs

**What goes wrong:** Job runs 45 minutes, cron fires every 30 minutes, next sleep is already negative.
**Why it happens:** `(fire_at - now).to_std()` underflows when `now > fire_at`.
**How to avoid:** After `execute_job` returns, recompute `next = schedule.after(&Utc::now()).next()` — do not reuse the pre-execution `fire_at`.

### Pitfall 4: Lock file left behind on panic

**What goes wrong:** Bot process panics mid-job, lock file stays, next run is permanently skipped until TTL expires.
**Why it happens:** `delete_lock()` call is after the subprocess returns — panic before that skips cleanup.
**How to avoid:** Use a drop guard (RAII) that deletes the lock file when dropped, even on unwinding. Or accept TTL-based recovery (30 minutes is fine for most jobs — document as expected behavior).

### Pitfall 5: `log_path` as absolute path stored in DB

**What goes wrong:** Agent moves its directory; stored `log_path` is stale.
**Why it happens:** D-04 says `log_path` is absolute.
**How to avoid:** Acceptable tradeoff per D-04. Agent directory does not move post-setup. Document as known limitation.

### Pitfall 6: Reconciler handle leak

**What goes wrong:** On each 60-second reload, old job handles are not aborted — jobs accumulate.
**Why it happens:** Forgetting to call `handle.abort()` on removed/changed jobs.
**How to avoid:** In the reconciler loop, explicitly `abort()` every handle not in the new spec set before inserting new handles.

---

## Code Examples

### Parsing a 5-field schedule via cron 0.16

```rust
// Source: docs.rs/cron/0.16, zslayton/cron README
use cron::Schedule;
use std::str::FromStr;
use chrono::Utc;

fn next_run_after(expr: &str) -> Option<chrono::DateTime<Utc>> {
    // Wrap 5-field → 7-field (sec + year fields required by cron 0.16)
    let seven_field = format!("0 {} *", expr.trim());
    let schedule = Schedule::from_str(&seven_field).ok()?;
    schedule.after(&Utc::now()).next()
}
```

### tokio::spawn cron task (from lib.rs)

```rust
// Source: crates/bot/src/lib.rs pattern
let cron_agent_dir = agent_dir.clone();
let cron_agent_name = args.agent.clone();
tokio::spawn(async move {
    cron::run_cron_task(cron_agent_dir, cron_agent_name).await;
});
```

### V3 migration registration

```rust
// Source: crates/rightclaw/src/memory/migrations.rs pattern
const V3_SCHEMA: &str = include_str!("sql/v3_cron_runs.sql");

pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> =
    std::sync::LazyLock::new(|| Migrations::new(vec![
        M::up(V1_SCHEMA),
        M::up(V2_SCHEMA),
        M::up(V3_SCHEMA),
    ]));
```

### MCP server name rename

```rust
// Source: rmcp 1.3.0 model.rs — with_server_info + Implementation
use rmcp::model::Implementation;

fn get_info(&self) -> ServerInfo {
    ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        .with_server_info(Implementation {
            name: "rightclaw".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        })
        .with_instructions("RightClaw tools: store, recall, search, forget, cron_list_runs, cron_show_run")
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| CC MCP CronCreate/CronDelete (skill-side) | Rust tokio cron task (bot-side) | Phase 27 | Cron no longer requires agent session; fires even when no chat active |
| Shell-side lock guard (prompt-wrapped logic) | Rust lock file check (before subprocess) | Phase 27 | Lock check is deterministic, not subject to CC hallucination |
| reconciler in skill (Python-like) | reconciler in Rust (JoinHandle map) | Phase 27 | No 3-day job expiry, no 50-task limit |

---

## Surprises and Gotchas Found in Codebase

1. **`agent_name` derivation**: In `handler.rs`, agent name is derived from `agent_dir.file_name()`. In `lib.rs`, `args.agent` is available directly. The cron task should use `args.agent` (already passed as `BotArgs`) — not re-derive from directory name.

2. **MCP server key in `.mcp.json` is `"rightmemory"`**: `mcp_config.rs` inserts the server under the key `"rightmemory"` in `mcpServers`. D-05 renames the _server name_ (`ServerInfo.server_info.name`) to `"rightclaw"` — this is what MCP protocol reports. The `.mcp.json` key (`"rightmemory"`) is the local identifier in the CC process — these are separate. D-05 says rename server name only; changing the `.mcp.json` key would be a separate breaking change not in scope.

3. **`rusqlite::Connection` is `!Send` but cron needs async**: Confirmed from `worker.rs` — each worker opens its own connection. Same approach for cron: `open_connection` per `execute_job` call.

4. **No `cron` crate in workspace yet**: The workspace `Cargo.toml` does not contain `cron`. Must add `cron = "0.16"` to `[workspace.dependencies]` and reference it from `crates/bot/Cargo.toml`.

5. **Log directory must be pre-created**: `crons/logs/` does not exist by default. `execute_job` must call `std::fs::create_dir_all(agent_dir.join("crons/logs"))` before writing the first log file. Similarly for `crons/.locks/`.

6. **`wait_with_output()` buffers all output**: For long-running jobs producing large output (think "morning briefing" gathering PRs), `wait_with_output()` holds stdout/stderr in memory until completion. This is fine for typical agent workloads (text output). Documented in `DIS-02` reasoning in `worker.rs`.

---

## Environment Availability

Step 2.6: SKIPPED — Phase 27 is a code change only. External dependencies (`claude`/`claude-bun` binary) are already checked at runtime by `which::which` (same as `invoke_cc`). No new external tools required.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in + `tempfile` for isolation |
| Config file | `Cargo.toml` workspace (no separate test config) |
| Quick run command | `cargo test -p rightclaw-bot -- cron` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | Notes |
|--------|----------|-----------|-------------------|-------|
| CRON-03 | 5→7 field expression wrapping | unit | `cargo test -p rightclaw-bot -- cron::tests::five_field_wrap` | Pure fn |
| CRON-03 | Schedule parse and next-run computation | unit | `cargo test -p rightclaw-bot -- cron::tests::next_run_after` | Needs `cron` dep |
| CRON-04 | Lock file freshness check | unit | `cargo test -p rightclaw-bot -- cron::tests::lock_check` | Pure fn, tempfile |
| CRON-04 | Stale lock is deleted and job proceeds | unit | `cargo test -p rightclaw-bot -- cron::tests::stale_lock_cleared` | tempfile |
| CRON-04 | lock_ttl parsing: m/h suffixes | unit | `cargo test -p rightclaw-bot -- cron::tests::parse_lock_ttl` | Pure fn |
| CRON-06 | Reconciler re-read doesn't duplicate handles | unit | `cargo test -p rightclaw-bot -- cron::tests::reconciler_idempotent` | In-memory map |
| D-04 | cron_runs V3 migration applied | unit | `cargo test -p rightclaw -- schema_has_cron_runs_table` | Extends mod.rs tests |
| D-05 | cron_list_runs returns rows sorted by started_at | unit | `cargo test -p rightclaw-cli -- cron_list_runs` | tempfile DB |
| D-05 | cron_show_run returns correct row | unit | `cargo test -p rightclaw-cli -- cron_show_run` | tempfile DB |

### Wave 0 Gaps

- [ ] `crates/bot/src/cron.rs` — entire new module (create in Wave 0 as stub with tests)
- [ ] `crates/rightclaw/src/memory/sql/v3_cron_runs.sql` — schema file
- [ ] `crates/rightclaw/src/memory/migrations.rs` — V3 addition + updated `user_version` test assertion

---

## Sources

### Primary (HIGH confidence)

- `crates/bot/src/lib.rs` — bot entry point, spawn insertion point confirmed
- `crates/bot/src/telegram/worker.rs` — `invoke_cc` pattern verified line-by-line
- `crates/bot/src/telegram/dispatch.rs` — signal/shutdown wiring confirmed
- `crates/rightclaw/src/memory/migrations.rs` + `sql/v1_schema.sql` + `sql/v2_telegram_sessions.sql` — migration pattern verified
- `crates/rightclaw-cli/src/memory_server.rs` — tool registration pattern, `#[tool]` derive pattern
- `crates/rightclaw/src/codegen/mcp_config.rs` — confirms `.mcp.json` key is `"rightmemory"` (separate from server name)
- `skills/cronsync/SKILL.md` — YAML spec fields, lock file format, lock_ttl default
- `.planning/REQUIREMENTS.md` — CRON-01..06, requirement text
- `.planning/phases/27-cron-runtime/27-CONTEXT.md` — all decisions D-01..D-06
- `~/.cargo/registry/.../rmcp-1.3.0/src/model.rs` — `Implementation::with_name`, `ServerInfo::with_server_info`
- docs.rs/cron/0.16.0 — 7-field expression requirement confirmed

### Secondary (MEDIUM confidence)

- zslayton/cron GitHub README — 7-field format `sec min hour dom mon dow year` confirmed
- crates.io `cargo search cron` — version 0.16.0 current

---

## Metadata

**Confidence breakdown:**
- Cron YAML spec: HIGH — read directly from SKILL.md
- Bot spawn pattern: HIGH — read lib.rs and dispatch.rs
- invoke_cc adaptation: HIGH — read worker.rs fully
- Migration infrastructure: HIGH — read migrations.rs + all SQL files
- MCP tool registration: HIGH — read memory_server.rs + rmcp source
- `cron` 0.16 API: HIGH — confirmed via docs.rs + GitHub README
- 5→7 field wrapping: HIGH — zslayton/cron README explicitly shows 7-field format

**Research date:** 2026-04-01
**Valid until:** 2026-05-01 (stable deps; `cron` 0.16 unlikely to change)
