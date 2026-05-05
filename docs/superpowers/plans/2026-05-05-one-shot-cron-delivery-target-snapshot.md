# One-shot cron delivery: snapshot target onto `cron_runs` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop dropping one-shot cron results on the floor. Snapshot `target_chat_id` and `target_thread_id` onto each `cron_runs` row at insert time, and read the delivery target from `cron_runs` directly instead of re-joining `cron_specs` (which is gone for one-shot crons by the time delivery runs).

**Architecture:** `cron_runs` becomes self-sufficient for delivery routing. Migration v18 adds nullable `target_chat_id`/`target_thread_id` columns to `cron_runs`. `CronSpec` gains both fields so they survive into `execute_job`, where the `INSERT INTO cron_runs … status='running'` writes them as a snapshot. `fetch_pending` and `deduplicate_job` in `cron_delivery.rs` drop their `LEFT JOIN cron_specs` and read target columns straight from `cron_runs`. Existing rows keep `NULL` and continue to map to `no_target` — the spec is gone, there is nowhere to recover the address from. `classify_pending_target` and `PendingCronResult` are unchanged: the public surface still presents `Option<i64>`, only the source-of-truth moves.

**Tech Stack:** Rust 2024, rusqlite, rusqlite_migration, anyhow/thiserror. No new dependencies.

---

## File Structure

- **Modify:** `crates/right-agent/src/memory/migrations.rs` — add `v18_cron_runs_target` hook and register in `MIGRATIONS`. Add a migration assertion test.
- **Modify:** `crates/right-agent/src/cron_spec.rs` — extend `CronSpec` with `target_chat_id` / `target_thread_id`; update `load_specs_from_db` to read them from `cron_specs`.
- **Modify:** `crates/bot/src/cron.rs` — propagate target fields from spec into the `INSERT INTO cron_runs … 'running'` statement in `execute_job`.
- **Modify:** `crates/bot/src/cron_delivery.rs` — switch `fetch_pending` and `deduplicate_job` SQL from `LEFT JOIN cron_specs` to direct reads of `cron_runs.target_chat_id` / `cron_runs.target_thread_id`. Update existing tests to seed targets on the run row, not the spec. Add a regression test that proves delivery survives spec deletion for one-shot jobs.

`PartialEq for CronSpec` already excludes transient state — adding target fields to the struct does not require touching `PartialEq` because they are immutable across a spec's life and *should* participate in equality (changing target via `cron_update` is a real change the reconciler should react to).

---

## Task 1: Add migration v18 — `target_chat_id`/`target_thread_id` on `cron_runs`

**Files:**
- Modify: `crates/right-agent/src/memory/migrations.rs:124-162` (hook + registration)
- Modify: `crates/right-agent/src/memory/migrations.rs:165-877` (test module)

- [ ] **Step 1: Write the failing migration test**

Append to the `tests` module in `crates/right-agent/src/memory/migrations.rs`:

```rust
    #[test]
    fn v18_cron_runs_has_target_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        let cols: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('cron_runs')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            cols.contains(&"target_chat_id".to_string()),
            "cron_runs.target_chat_id column missing"
        );
        assert!(
            cols.contains(&"target_thread_id".to_string()),
            "cron_runs.target_thread_id column missing"
        );
    }

    #[test]
    fn v18_is_idempotent() {
        // Apply once.
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_latest(&mut conn).unwrap();
        // Apply again — must not error (re-running should be a no-op).
        MIGRATIONS.to_latest(&mut conn).unwrap();
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p right-agent --lib memory::migrations::tests::v18 -- --nocapture`
Expected: FAIL — `cron_runs.target_chat_id column missing`.

- [ ] **Step 3: Implement migration v18**

Add after `v17_cron_target` in `crates/right-agent/src/memory/migrations.rs`:

```rust
/// v18: Add target_chat_id and target_thread_id to cron_runs.
///
/// Snapshot of the spec's delivery target taken at run-insert time. Lets the
/// delivery loop find the recipient even after a one-shot spec auto-deletes.
/// Idempotent — checks pragma_table_info before each ALTER. Both columns are
/// nullable; existing rows stay NULL (their spec is already gone — no recovery
/// path) and continue to surface as `delivery_status='no_target'`.
fn v18_cron_runs_target(tx: &Transaction) -> Result<(), HookError> {
    let has_column = |col: &str| -> Result<bool, rusqlite::Error> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('cron_runs') WHERE name = ?1",
            [col],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    };

    if !has_column("target_chat_id")? {
        tx.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_chat_id INTEGER")?;
    }
    if !has_column("target_thread_id")? {
        tx.execute_batch("ALTER TABLE cron_runs ADD COLUMN target_thread_id INTEGER")?;
    }
    Ok(())
}
```

Then register it in the `MIGRATIONS` list (existing code at line ~143):

```rust
pub static MIGRATIONS: std::sync::LazyLock<Migrations<'static>> = std::sync::LazyLock::new(|| {
    Migrations::new(vec![
        M::up(V1_SCHEMA),
        M::up(V2_SCHEMA),
        M::up(V3_SCHEMA),
        M::up(V4_SCHEMA),
        M::up(V5_SCHEMA),
        M::up(V6_SCHEMA),
        M::up(V7_SCHEMA),
        M::up(V8_SCHEMA),
        M::up(V9_SCHEMA),
        M::up(V10_SCHEMA),
        M::up(V11_SCHEMA),
        M::up_with_hook("", v12_cron_diagnostics),
        M::up_with_hook("", v13_one_shot_cron),
        M::up(V14_SCHEMA),
        M::up(V15_SCHEMA),
        M::up_with_hook("", v16_usage_api_key_source),
        M::up_with_hook("", v17_cron_target),
        M::up_with_hook("", v18_cron_runs_target),
    ])
});
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p right-agent --lib memory::migrations::tests::v18 -- --nocapture`
Expected: PASS for both `v18_cron_runs_has_target_columns` and `v18_is_idempotent`.

- [ ] **Step 5: Run the full migrations test module to confirm no regressions**

Run: `cargo test -p right-agent --lib memory::migrations`
Expected: PASS, all migration tests green.

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/memory/migrations.rs
git commit -m "feat(cron): migrate cron_runs to carry target_chat_id/target_thread_id (v18)"
```

---

## Task 2: Carry target fields on `CronSpec`

**Files:**
- Modify: `crates/right-agent/src/cron_spec.rs:62-71` (struct definition)
- Modify: `crates/right-agent/src/cron_spec.rs:551-603` (`load_specs_from_db`)
- Modify: `crates/right-agent/src/cron_spec.rs:1166+` (test module — `load_specs_from_db_returns_all` + new test)

- [ ] **Step 1: Write the failing test**

Append to the `tests` module of `crates/right-agent/src/cron_spec.rs`:

```rust
    #[test]
    fn load_specs_from_db_carries_target_fields() {
        let (_tmp, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('with-target', '*/5 * * * *', 'p', NULL, 1.0, 1, -555, 9, ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, created_at, updated_at) \
             VALUES ('no-target', '*/5 * * * *', 'p', NULL, 1.0, 1, ?1, ?1)",
            [&now],
        )
        .unwrap();

        let specs = load_specs_from_db(&conn).unwrap();
        let with = &specs["with-target"];
        assert_eq!(with.target_chat_id, Some(-555));
        assert_eq!(with.target_thread_id, Some(9));

        let without = &specs["no-target"];
        assert_eq!(without.target_chat_id, None);
        assert_eq!(without.target_thread_id, None);
    }
```

`setup_db` already exists in this test module (search for `fn setup_db`); it returns a tempdir + migrated connection.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p right-agent --lib cron_spec::tests::load_specs_from_db_carries_target_fields -- --nocapture`
Expected: FAIL — `no field 'target_chat_id' on type 'CronSpec'`.

- [ ] **Step 3: Add the fields to `CronSpec`**

Edit `crates/right-agent/src/cron_spec.rs:62-71`:

```rust
/// A cron job specification loaded from the database.
#[derive(Debug, Clone)]
pub struct CronSpec {
    pub schedule_kind: ScheduleKind,
    pub prompt: String,
    pub lock_ttl: Option<String>,
    pub max_budget_usd: f64,
    pub triggered_at: Option<String>,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
}
```

`PartialEq` (line 78-86) already only compares `schedule_kind`, `prompt`, `lock_ttl`, `max_budget_usd`. Extend it to include the new fields so the reconciler picks up `cron_update` calls that change targets:

```rust
impl PartialEq for CronSpec {
    fn eq(&self, other: &Self) -> bool {
        self.schedule_kind == other.schedule_kind
            && self.prompt == other.prompt
            && self.lock_ttl == other.lock_ttl
            && self.max_budget_usd == other.max_budget_usd
            && self.target_chat_id == other.target_chat_id
            && self.target_thread_id == other.target_thread_id
    }
}
```

- [ ] **Step 4: Update `load_specs_from_db` to read target columns**

Replace the body of `load_specs_from_db` (lines 551-603 in `crates/right-agent/src/cron_spec.rs`):

```rust
pub fn load_specs_from_db(
    conn: &rusqlite::Connection,
) -> Result<HashMap<String, CronSpec>, rusqlite::Error> {
    let mut specs = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT job_name, schedule, prompt, lock_ttl, max_budget_usd, triggered_at, recurring, run_at, target_chat_id, target_thread_id FROM cron_specs",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<i64>>(9)?,
        ))
    })?;

    for row in rows {
        let (
            job_name,
            schedule,
            prompt,
            lock_ttl,
            max_budget_usd,
            triggered_at,
            recurring,
            run_at,
            target_chat_id,
            target_thread_id,
        ) = row?;

        let schedule_kind = if let Some(ref rat) = run_at {
            match rat.parse::<DateTime<Utc>>() {
                Ok(dt) => ScheduleKind::RunAt(dt),
                Err(e) => {
                    tracing::error!(job = %job_name, "invalid run_at in DB: {e:#}");
                    continue;
                }
            }
        } else if recurring == 0 {
            ScheduleKind::OneShotCron(schedule)
        } else {
            ScheduleKind::Recurring(schedule)
        };

        specs.insert(
            job_name,
            CronSpec {
                schedule_kind,
                prompt,
                lock_ttl,
                max_budget_usd,
                triggered_at,
                target_chat_id,
                target_thread_id,
            },
        );
    }

    Ok(specs)
}
```

- [ ] **Step 5: Fix all other `CronSpec { … }` literals in this file**

Other places construct `CronSpec` (notably `load_specs_from_db_returns_all` and the equality tests `triggered_at_does_not_affect_equality` / `spec_equality_detects_real_changes`, plus any production constructor — search the file). Each must add the two new fields. Run:

```bash
rg -n "CronSpec \{" crates/right-agent/src/cron_spec.rs
```

For every match, add `target_chat_id: None, target_thread_id: None,` to the base literal. The compiler lists them all if you miss any.

While you are in `spec_equality_detects_real_changes` (around `crates/right-agent/src/cron_spec.rs:1131-1154`), append a target-change case so the new equality contribution is exercised:

```rust
        let changed_target = CronSpec {
            target_chat_id: Some(-12345),
            ..base.clone()
        };
        assert_ne!(base, changed_target, "target_chat_id change must be a real change");
```

- [ ] **Step 6: Run the cron_spec tests**

Run: `cargo test -p right-agent --lib cron_spec`
Expected: PASS, including the new `load_specs_from_db_carries_target_fields` test.

- [ ] **Step 7: Build the workspace to catch downstream breakage**

Run: `cargo build --workspace`
Expected: PASS. If anything outside `cron_spec.rs` constructs `CronSpec` literally, it breaks here — add the two new fields at each site (likely tests in `crates/bot/`).

- [ ] **Step 8: Commit**

```bash
git add crates/right-agent/src/cron_spec.rs
git commit -m "feat(cron): carry target_chat_id/target_thread_id on CronSpec"
```

If Step 7 forced you to touch other crates, include those files in the same commit.

---

## Task 3: Snapshot target onto `cron_runs` at execute_job insert

**Files:**
- Modify: `crates/bot/src/cron.rs:266-283` (the `INSERT INTO cron_runs … 'running'` site)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod` in `crates/bot/src/cron.rs` (search for `mod classify_tests` at line ~1170 — add the test in that module or in a sibling test module that matches existing patterns). Add a focused unit test that exercises the INSERT in isolation. Easiest path: extract the INSERT into a small helper and test the helper, OR test via integration by triggering a job and reading `cron_runs`. Pick the helper-extraction approach to keep the test fast and hermetic.

First, write the test that calls a not-yet-existing helper:

```rust
#[cfg(test)]
mod target_snapshot_tests {
    use super::*;
    use right_agent::cron_spec::{CronSpec, ScheduleKind};

    fn migrated_conn() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = right_agent::memory::open_connection(dir.path(), true).unwrap();
        (dir, conn)
    }

    #[test]
    fn insert_running_run_snapshots_target() {
        let (_dir, conn) = migrated_conn();
        let spec = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            target_chat_id: Some(-777),
            target_thread_id: Some(13),
        };
        insert_running_run(
            &conn,
            "run-1",
            "job-x",
            "2026-05-05T12:00:00Z",
            "/log/path",
            &spec,
        )
        .unwrap();

        let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_runs WHERE id = 'run-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(chat, Some(-777));
        assert_eq!(thread, Some(13));
    }

    #[test]
    fn insert_running_run_writes_null_when_spec_has_no_target() {
        let (_dir, conn) = migrated_conn();
        let spec = CronSpec {
            schedule_kind: ScheduleKind::Recurring("*/5 * * * *".into()),
            prompt: "p".into(),
            lock_ttl: None,
            max_budget_usd: 1.0,
            triggered_at: None,
            target_chat_id: None,
            target_thread_id: None,
        };
        insert_running_run(
            &conn,
            "run-2",
            "job-y",
            "2026-05-05T12:00:00Z",
            "/log/path",
            &spec,
        )
        .unwrap();

        let (chat, thread): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT target_chat_id, target_thread_id FROM cron_runs WHERE id = 'run-2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(chat, None);
        assert_eq!(thread, None);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p right-bot --lib cron::target_snapshot_tests -- --nocapture`
Expected: FAIL — `cannot find function 'insert_running_run'`.

- [ ] **Step 3: Extract the helper and call it from `execute_job`**

In `crates/bot/src/cron.rs`, add a helper directly above `execute_job` (around line 209):

```rust
/// Insert a freshly-started cron run with `status='running'`, snapshotting the
/// spec's delivery target onto the row so one-shot delivery survives spec
/// auto-deletion.
fn insert_running_run(
    conn: &rusqlite::Connection,
    run_id: &str,
    job_name: &str,
    started_at: &str,
    log_path: &str,
    spec: &right_agent::cron_spec::CronSpec,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, target_chat_id, target_thread_id) \
         VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6)",
        rusqlite::params![
            run_id,
            job_name,
            started_at,
            log_path,
            spec.target_chat_id,
            spec.target_thread_id,
        ],
    )?;
    Ok(())
}
```

Then replace the inline INSERT in `execute_job` (currently `crates/bot/src/cron.rs:276-283`):

```rust
    if let Err(e) = insert_running_run(
        &conn,
        &run_id,
        job_name,
        &started_at,
        &log_path_str,
        spec,
    ) {
        tracing::error!(job = %job_name, "DB insert failed: {e:#}");
        std::fs::remove_file(&lock_path).ok();
        return;
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p right-bot --lib cron::target_snapshot_tests -- --nocapture`
Expected: PASS for both new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): snapshot target_chat_id/target_thread_id onto cron_runs"
```

---

## Task 4: Read target from `cron_runs` in delivery — drop the `LEFT JOIN`

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:24-52` (`fetch_pending`)
- Modify: `crates/bot/src/cron_delivery.rs:72-114` (`deduplicate_job`)
- Modify: `crates/bot/src/cron_delivery.rs:888-925` (existing `fetch_pending_carries_target_fields` and `fetch_pending_returns_none_target_when_spec_has_none` tests — they currently seed targets on the spec and rely on JOIN)

- [ ] **Step 1: Write the regression test that proves we survive spec deletion**

Append to the `tests` module of `crates/bot/src/cron_delivery.rs`:

```rust
    #[test]
    fn fetch_pending_resolves_target_after_spec_deletion() {
        // Reproduces the production bug: a one-shot spec auto-deletes after
        // firing, but the run row still needs to know where to deliver.
        let (_dir, conn) = setup_db();
        let now = chrono::Utc::now().to_rfc3339();

        // 1. Spec is created (recurring=0, one-shot).
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, recurring, target_chat_id, target_thread_id, created_at, updated_at) \
             VALUES ('one-shot', '*/5 * * * *', 'p', 1.0, 0, -4996137249, NULL, ?1, ?1)",
            [&now],
        ).unwrap();

        // 2. Run row inserted with snapshot of target (what new execute_job does).
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, target_chat_id, target_thread_id) \
             VALUES ('run-1', 'one-shot', '2026-05-05T12:36:00Z', '2026-05-05T12:41:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}', -4996137249, NULL)",
            [],
        ).unwrap();

        // 3. Spec is auto-deleted (one-shot completion).
        conn.execute("DELETE FROM cron_specs WHERE job_name = 'one-shot'", [])
            .unwrap();

        // 4. Delivery loop fetches — must still find the target.
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-4996137249));
        assert_eq!(pending.target_thread_id, None);

        // And dedup must agree.
        let (latest, _skipped) = deduplicate_job(&conn, "one-shot").unwrap().unwrap();
        assert_eq!(latest.target_chat_id, Some(-4996137249));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p right-bot --lib cron_delivery::tests::fetch_pending_resolves_target_after_spec_deletion -- --nocapture`
Expected: FAIL — `assertion failed: pending.target_chat_id == Some(-4996137249)` (current JOIN finds nothing because the spec row is gone).

- [ ] **Step 3: Switch `fetch_pending` to read targets from `cron_runs`**

Replace `fetch_pending` in `crates/bot/src/cron_delivery.rs:24-52`:

```rust
/// Query the oldest undelivered cron result with a non-null notify_json.
pub fn fetch_pending(
    conn: &rusqlite::Connection,
) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, notify_json, summary, finished_at, status, target_chat_id, target_thread_id \
         FROM cron_runs \
         WHERE status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at ASC LIMIT 1",
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
            status: row.get(5)?,
            target_chat_id: row.get(6)?,
            target_thread_id: row.get(7)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}
```

- [ ] **Step 4: Switch `deduplicate_job` to read targets from `cron_runs`**

In `crates/bot/src/cron_delivery.rs` (the SELECT inside `deduplicate_job` around line 76-97), drop the JOIN:

```rust
    let latest = conn
        .query_row(
            "SELECT id, job_name, notify_json, summary, finished_at, status, target_chat_id, target_thread_id \
             FROM cron_runs \
             WHERE job_name = ?1 AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL \
             ORDER BY finished_at DESC LIMIT 1",
            rusqlite::params![job_name],
            |row| {
                Ok(PendingCronResult {
                    id: row.get(0)?,
                    job_name: row.get(1)?,
                    notify_json: row.get(2)?,
                    summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    status: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    target_chat_id: row.get(6)?,
                    target_thread_id: row.get(7)?,
                })
            },
        )
        .optional()?;
```

(Leave the rest of `deduplicate_job` — older-run marking, return shape — unchanged.)

- [ ] **Step 5: Update existing tests to seed target on the run row, not the spec**

`fetch_pending_carries_target_fields` (lines ~888-906 in `crates/bot/src/cron_delivery.rs`) and `fetch_pending_returns_none_target_when_spec_has_none` (lines ~908-925) both currently seed target on `cron_specs` and rely on JOIN. They must move targets onto the run row. Replace them:

```rust
    #[test]
    fn fetch_pending_carries_target_fields() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, target_chat_id, target_thread_id) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}', -555, 9)",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.target_chat_id, Some(-555));
        assert_eq!(pending.target_thread_id, Some(9));
    }

    #[test]
    fn fetch_pending_returns_none_target_when_run_has_none() {
        let (_dir, conn) = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'legacy', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert!(pending.target_chat_id.is_none());
        assert!(pending.target_thread_id.is_none());
    }
```

(The renamed test `_when_run_has_none` reflects the new source of truth. Keep `null_target_classifies_as_no_target` from `crates/bot/src/cron_delivery.rs:928` unchanged in spirit but check whether its setup also seeded the spec — if so, move the NULL target into the run row analogously.)

- [ ] **Step 6: Audit other test setups in cron_delivery that seed target on the spec**

Run: `rg -n "INSERT INTO cron_specs" crates/bot/src/cron_delivery.rs`

For each match: if the test asserts on `pending.target_chat_id` (or routes through `classify_pending_target`), move target columns onto the corresponding `cron_runs` insert and drop them from the spec insert (or remove the spec insert entirely if it was only there for the JOIN). If a test does not depend on target resolution, leave it alone.

- [ ] **Step 7: Run the cron_delivery tests**

Run: `cargo test -p right-bot --lib cron_delivery`
Expected: PASS, including `fetch_pending_resolves_target_after_spec_deletion`, both updated `fetch_pending_*` tests, and any other tests touched in Step 6.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "fix(cron): read delivery target from cron_runs, drop JOIN to cron_specs"
```

---

## Task 5: Workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Build the whole workspace**

Run: `cargo build --workspace`
Expected: PASS, no warnings about unused fields in `CronSpec` etc.

- [ ] **Step 2: Run the full test suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Final manual verification on a live agent (optional, recommended)**

After deploying this build:

1. From a chat the agent listens to: ask the agent to schedule a one-shot cron 1 minute from now (e.g. `cron_create job_name=delivery-smoke recurring=false run_at=<now+1m> prompt="reply with a one-line ping"`).
2. Wait ~3 minutes.
3. Confirm the agent posted the ping reply in the chat.
4. Confirm `sqlite3 ~/.right/agents/<name>/data.db "SELECT delivery_status FROM cron_runs WHERE job_name='delivery-smoke';"` shows `delivered`.

---

## Out of scope

- **Recovery for existing `no_target` rows.** The 3049-byte `design-studio-scoring` payload (and `notion-master-db-migration` from 2026-04-22) are already orphaned: their specs are gone and the rows have NULL targets. There is no automatic recovery path because the original target is unrecoverable. The user can ask the agent to read `cron_show_run` for the relevant run_id and re-post the body manually if they still want it. A one-off `UPDATE cron_runs SET target_chat_id = X, delivered_at = NULL, delivery_status = NULL WHERE id = '…'` is technically possible after this fix lands but is a manual operator action, not part of this plan.
- **Doctor checks.** `doctor::check_cron_targets` already warns on specs with NULL targets — that is still correct (a recurring spec without a target is broken). No changes needed.
- **`cron_update` propagating to in-flight runs.** A target change via `cron_update` after the run has already INSERTed cron_runs will not retroactively update the snapshot. This is intentional: a run is delivered to whoever the spec said at start time. If the user wants a different chat, they can `cron_update` and the *next* run picks it up.
