# Cron Self-Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let agents diagnose cron delivery issues by exposing `delivery_status`, `no_notify_reason`, and `delivered_at` through MCP tools.

**Architecture:** DB migration adds two columns (`delivery_status`, `no_notify_reason`) to `cron_runs`. Cron execution sets `silent`/`pending` + reason. Delivery loop transitions to `delivered`/`superseded`/`failed`. MCP output includes all three new fields. JSON schema updated so CC explains silent runs.

**Tech Stack:** Rust, SQLite (rusqlite_migration), serde, rmcp

---

### Task 1: DB migration — add `delivery_status` and `no_notify_reason` columns

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v12_cron_diagnostics.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs:1-30`

- [ ] **Step 1: Write the migration test**

Add to `crates/rightclaw/src/memory/migrations.rs` inside the `mod tests` block, after the `v10_mcp_servers_has_auth_columns` test:

```rust
#[test]
fn v12_cron_diagnostics_columns() {
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
        cols.contains(&"delivery_status".to_string()),
        "delivery_status column missing"
    );
    assert!(
        cols.contains(&"no_notify_reason".to_string()),
        "no_notify_reason column missing"
    );
}

#[test]
fn v12_backfill_delivery_status() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    // Insert a delivered run (has notify_json + delivered_at)
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json, delivered_at) \
         VALUES ('d1', 'j1', '2026-01-01T00:00:00Z', 'success', '/log', '{\"content\":\"hi\"}', '2026-01-01T00:05:00Z')",
        [],
    ).unwrap();
    // Insert a pending run (has notify_json, no delivered_at)
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, notify_json) \
         VALUES ('p1', 'j1', '2026-01-01T01:00:00Z', 'success', '/log', '{\"content\":\"pending\"}')",
        [],
    ).unwrap();
    // Insert a silent run (no notify_json)
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary) \
         VALUES ('s1', 'j1', '2026-01-01T02:00:00Z', 'success', '/log', 'quiet')",
        [],
    ).unwrap();

    let status_of = |id: &str| -> Option<String> {
        conn.query_row(
            "SELECT delivery_status FROM cron_runs WHERE id = ?1",
            [id],
            |r| r.get(0),
        ).unwrap()
    };
    assert_eq!(status_of("d1").as_deref(), Some("delivered"));
    assert_eq!(status_of("p1").as_deref(), Some("pending"));
    assert_eq!(status_of("s1").as_deref(), Some("silent"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw v12_ -- --nocapture 2>&1 | tail -20`
Expected: FAIL — migration v12 does not exist yet.

- [ ] **Step 3: Create the migration SQL file**

Create `crates/rightclaw/src/memory/sql/v12_cron_diagnostics.sql`:

```sql
-- v12: Add delivery_status and no_notify_reason to cron_runs for self-diagnostics.
-- delivery_status tracks lifecycle: silent, pending, delivered, superseded, failed.
-- no_notify_reason stores CC's explanation when notify is null.
ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT;
ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT;

-- Backfill existing rows based on current state.
UPDATE cron_runs SET delivery_status = 'delivered'
  WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
UPDATE cron_runs SET delivery_status = 'pending'
  WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
UPDATE cron_runs SET delivery_status = 'silent'
  WHERE notify_json IS NULL;
```

- [ ] **Step 4: Register the migration**

In `crates/rightclaw/src/memory/migrations.rs`, add after line 13 (`const V11_SCHEMA`):

```rust
const V12_SCHEMA: &str = include_str!("sql/v12_cron_diagnostics.sql");
```

And add `M::up(V12_SCHEMA),` after `M::up(V11_SCHEMA),` in the `Migrations::new` vec (after line 28).

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw v12_ -- --nocapture 2>&1 | tail -20`
Expected: PASS — both `v12_cron_diagnostics_columns` and `v12_backfill_delivery_status` pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v12_cron_diagnostics.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(cron): add delivery_status and no_notify_reason columns (v12 migration)"
```

---

### Task 2: Update `CRON_SCHEMA_JSON` and `CronReplyOutput`

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:29-33`
- Modify: `crates/bot/src/cron.rs:32-37`

- [ ] **Step 1: Write test for `no_notify_reason` parsing**

Add to `crates/bot/src/cron.rs` in the `mod tests` block, after the `parse_cron_output_silent_null_notify` test:

```rust
#[test]
fn parse_cron_output_silent_with_reason() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":null,"summary":"Nothing interesting","no_notify_reason":"No changes since last run"}}"#.to_string(),
    ];
    let out = parse_cron_output(&lines).unwrap();
    assert!(out.notify.is_none());
    assert_eq!(out.summary, "Nothing interesting");
    assert_eq!(out.no_notify_reason.as_deref(), Some("No changes since last run"));
}

#[test]
fn parse_cron_output_notify_present_no_reason() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","structured_output":{"notify":{"content":"BTC broke 100k"},"summary":"Checked pairs","no_notify_reason":null}}"#.to_string(),
    ];
    let out = parse_cron_output(&lines).unwrap();
    assert!(out.notify.is_some());
    assert!(out.no_notify_reason.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p rightclaw-bot parse_cron_output_silent_with_reason -- --nocapture 2>&1 | tail -20`
Expected: FAIL — `CronReplyOutput` has no field `no_notify_reason`.

- [ ] **Step 3: Add `no_notify_reason` to `CronReplyOutput`**

In `crates/bot/src/cron.rs`, change the struct at lines 33-37 from:

```rust
pub struct CronReplyOutput {
    pub notify: Option<CronNotify>,
    pub summary: String,
}
```

to:

```rust
pub struct CronReplyOutput {
    pub notify: Option<CronNotify>,
    pub summary: String,
    pub no_notify_reason: Option<String>,
}
```

- [ ] **Step 4: Update `CRON_SCHEMA_JSON`**

In `crates/rightclaw/src/codegen/agent_def.rs`, replace the doc comment and constant at lines 29-33. The new doc comment:

```rust
/// JSON schema for cron job structured output.
///
/// `summary` is always required. `notify` is null when the cron ran silently
/// (no user notification needed). When `notify` is present, `content` is required.
/// `no_notify_reason` is required when `notify` is null — a short factual explanation
/// of why there is nothing to report (e.g. "No changes since last run").
```

The new constant value (single line, same style as existing):

```rust
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":["object","null"],"properties":{"content":{"type":"string"},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"},"no_notify_reason":{"type":["string","null"]}},"required":["summary"]}"#;
```

The only change is adding `,"no_notify_reason":{"type":["string","null"]}` after the `"summary"` property.

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw-bot parse_cron_output_silent_with_reason parse_cron_output_notify_present_no_reason -- --nocapture 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/bot/src/cron.rs
git commit -m "feat(cron): add no_notify_reason to schema and CronReplyOutput"
```

---

### Task 3: Set `delivery_status` and `no_notify_reason` during cron execution

**Files:**
- Modify: `crates/bot/src/cron.rs:385-468` (the post-execution persist block)

- [ ] **Step 1: Update the DB persist query for successful runs with notify**

In `crates/bot/src/cron.rs`, find the UPDATE statement at line 453-455:

```rust
if let Err(e) = conn.execute(
    "UPDATE cron_runs SET summary = ?1, notify_json = ?2 WHERE id = ?3",
    rusqlite::params![cron_output.summary, notify_json, run_id],
) {
```

Replace with:

```rust
let delivery_status = if cron_output.notify.is_some() {
    "pending"
} else {
    "silent"
};
if let Err(e) = conn.execute(
    "UPDATE cron_runs SET summary = ?1, notify_json = ?2, delivery_status = ?3, no_notify_reason = ?4 WHERE id = ?5",
    rusqlite::params![cron_output.summary, notify_json, delivery_status, cron_output.no_notify_reason, run_id],
) {
```

- [ ] **Step 2: Update the tracing log to include new fields**

In the same file, update the `tracing::info!` at lines 460-464 from:

```rust
tracing::info!(
    job = %job_name,
    has_notify = cron_output.notify.is_some(),
    "cron output persisted to DB"
);
```

to:

```rust
tracing::info!(
    job = %job_name,
    has_notify = cron_output.notify.is_some(),
    delivery_status,
    no_notify_reason = cron_output.no_notify_reason.as_deref().unwrap_or("-"),
    "cron output persisted to DB"
);
```

- [ ] **Step 3: Update the failure branch persist**

In the same file, find the failure persist block at lines 470-500. After building `let content = format!(...)` and `let notify = CronNotify { ... }`, find the UPDATE at line 491:

```rust
"UPDATE cron_runs SET summary = ?1, notify_json = ?2 WHERE id = ?3",
```

Replace with:

```rust
"UPDATE cron_runs SET summary = ?1, notify_json = ?2, delivery_status = 'pending' WHERE id = ?3",
```

Failure notifications always get `delivery_status = 'pending'` because they always have `notify` set.

- [ ] **Step 4: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | tail -20`
Expected: No errors.

- [ ] **Step 5: Run all cron tests**

Run: `devenv shell -- cargo test -p rightclaw-bot cron -- --nocapture 2>&1 | tail -30`
Expected: All existing and new tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): set delivery_status and no_notify_reason during execution"
```

---

### Task 4: Set `delivery_status` during delivery lifecycle

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:58-95` (deduplicate_job)
- Modify: `crates/bot/src/cron_delivery.rs:270-311` (delivery success/failure handlers in run_delivery_loop)

- [ ] **Step 1: Write test for dedup setting `superseded` status**

Add to `crates/bot/src/cron_delivery.rs` in `mod tests`, after the `deduplicate_does_not_touch_other_jobs` test:

```rust
#[test]
fn deduplicate_sets_superseded_status() {
    let (_dir, conn) = setup_db();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivery_status) \
         VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"old\"}', 'pending')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivery_status) \
         VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"new\"}', 'pending')",
        [],
    ).unwrap();
    let (latest, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
    assert_eq!(latest.id, "b");
    assert_eq!(skipped, 1);

    let status: Option<String> = conn
        .query_row(
            "SELECT delivery_status FROM cron_runs WHERE id = 'a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status.as_deref(), Some("superseded"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw-bot deduplicate_sets_superseded -- --nocapture 2>&1 | tail -20`
Expected: FAIL — dedup currently does not set `delivery_status`.

- [ ] **Step 3: Update `deduplicate_job` to set `superseded`**

In `crates/bot/src/cron_delivery.rs`, change the UPDATE at lines 87-91 from:

```rust
let count = conn.execute(
    "UPDATE cron_runs SET delivered_at = ?1 \
     WHERE job_name = ?2 AND id != ?3 \
     AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL",
    rusqlite::params![now, job_name, latest.id],
)?;
```

to:

```rust
let count = conn.execute(
    "UPDATE cron_runs SET delivered_at = ?1, delivery_status = 'superseded' \
     WHERE job_name = ?2 AND id != ?3 \
     AND status IN ('success', 'failed') AND notify_json IS NOT NULL AND delivered_at IS NULL",
    rusqlite::params![now, job_name, latest.id],
)?;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `devenv shell -- cargo test -p rightclaw-bot deduplicate_sets_superseded -- --nocapture 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 5: Set `delivered` on successful delivery**

In the `run_delivery_loop` function in `crates/bot/src/cron_delivery.rs`, find the success branch at line 271-275:

```rust
Ok(()) => {
    if let Err(e) = mark_delivered(&conn, &to_deliver.id) {
```

Add a `delivery_status` update right before `mark_delivered`. Change the block to:

```rust
Ok(()) => {
    if let Err(e) = conn.execute(
        "UPDATE cron_runs SET delivery_status = 'delivered' WHERE id = ?1",
        rusqlite::params![to_deliver.id],
    ) {
        tracing::error!(run_id = %to_deliver.id, "failed to set delivery_status=delivered: {e:#}");
    }
    if let Err(e) = mark_delivered(&conn, &to_deliver.id) {
```

- [ ] **Step 6: Set `failed` on max retry exhaustion**

In the same function, find the failure branch where max attempts is reached, at lines 298-308:

```rust
if *attempts >= MAX_DELIVERY_ATTEMPTS {
    tracing::warn!(
        job = %to_deliver.job_name,
        run_id = %to_deliver.id,
        "giving up after {MAX_DELIVERY_ATTEMPTS} attempts, marking as delivered"
    );
    if let Err(db_err) = mark_delivered(&conn, &to_deliver.id) {
```

Add a `delivery_status` update before `mark_delivered`. Change to:

```rust
if *attempts >= MAX_DELIVERY_ATTEMPTS {
    tracing::warn!(
        job = %to_deliver.job_name,
        run_id = %to_deliver.id,
        "giving up after {MAX_DELIVERY_ATTEMPTS} attempts, marking as delivered"
    );
    if let Err(e) = conn.execute(
        "UPDATE cron_runs SET delivery_status = 'failed' WHERE id = ?1",
        rusqlite::params![to_deliver.id],
    ) {
        tracing::error!(run_id = %to_deliver.id, "failed to set delivery_status=failed: {e:#}");
    }
    if let Err(db_err) = mark_delivered(&conn, &to_deliver.id) {
```

- [ ] **Step 7: Verify it compiles**

Run: `devenv shell -- cargo check -p rightclaw-bot 2>&1 | tail -20`
Expected: No errors.

- [ ] **Step 8: Run all delivery tests**

Run: `devenv shell -- cargo test -p rightclaw-bot cron_delivery -- --nocapture 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron): set delivery_status during delivery lifecycle"
```

---

### Task 5: Expose new fields in MCP output

**Files:**
- Modify: `crates/rightclaw-cli/src/memory_server.rs:203-285` (SQL queries + `cron_run_to_json`)
- Modify: `crates/rightclaw-cli/src/right_backend.rs:331-400` (SQL queries)

- [ ] **Step 1: Write test for new fields in MCP output**

Add to `crates/rightclaw-cli/src/memory_server_mcp_tests.rs`, after the `test_cron_list_runs_limit` test:

```rust
#[tokio::test]
async fn test_cron_list_runs_includes_diagnostics_fields() {
    let (server, _dir) = setup_server();
    let conn = server.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary, delivery_status, no_notify_reason) \
         VALUES ('diag-1', 'tracker', '2026-04-01T10:00:00Z', 'success', '/log', 'quiet', 'silent', 'No changes since last run')",
        [],
    ).expect("insert");
    conn.execute(
        "INSERT INTO cron_runs (id, job_name, started_at, status, log_path, summary, notify_json, delivery_status, delivered_at) \
         VALUES ('diag-2', 'tracker', '2026-04-01T11:00:00Z', 'success', '/log', 'found stuff', '{\"content\":\"new release\"}', 'delivered', '2026-04-01T11:05:00Z')",
        [],
    ).expect("insert");
    drop(conn);

    let result = server
        .cron_list_runs(Parameters(CronListRunsParams {
            job_name: Some("tracker".to_string()),
            limit: None,
        }))
        .await
        .expect("cron_list_runs ok");
    let text = call_result_text(result);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed.len(), 2);

    // diag-2 is first (DESC order)
    assert_eq!(parsed[0]["delivery_status"], "delivered");
    assert_eq!(parsed[0]["delivered_at"], "2026-04-01T11:05:00Z");
    assert!(parsed[0]["no_notify_reason"].is_null());

    // diag-1 is second
    assert_eq!(parsed[1]["delivery_status"], "silent");
    assert_eq!(parsed[1]["no_notify_reason"], "No changes since last run");
    assert!(parsed[1]["delivered_at"].is_null());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p rightclaw-cli test_cron_list_runs_includes_diagnostics -- --nocapture 2>&1 | tail -20`
Expected: FAIL — `cron_run_to_json` does not include the new fields.

- [ ] **Step 3: Update `cron_run_to_json` signature and body**

In `crates/rightclaw-cli/src/memory_server.rs`, change `cron_run_to_json` at lines 472-502 from:

```rust
pub(crate) fn cron_run_to_json(
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    log_path: Option<&str>,
    summary: Option<&str>,
    notify_json: Option<&str>,
) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": id,
        "job_name": job_name,
        "started_at": started_at,
        "finished_at": finished_at,
        "exit_code": exit_code,
        "status": status,
        "log_path": log_path,
    });
    if let Some(s) = summary {
        val["summary"] = serde_json::Value::String(s.to_owned());
    }
    // Parse notify_json into a structured object so the agent sees content directly.
    if let Some(nj) = notify_json {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(nj) {
            val["notify"] = parsed;
        }
    }
    val
}
```

to:

```rust
pub(crate) fn cron_run_to_json(
    id: &str,
    job_name: &str,
    started_at: &str,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
    status: &str,
    log_path: Option<&str>,
    summary: Option<&str>,
    notify_json: Option<&str>,
    delivered_at: Option<&str>,
    delivery_status: Option<&str>,
    no_notify_reason: Option<&str>,
) -> serde_json::Value {
    let mut val = serde_json::json!({
        "id": id,
        "job_name": job_name,
        "started_at": started_at,
        "finished_at": finished_at,
        "exit_code": exit_code,
        "status": status,
        "log_path": log_path,
        "delivered_at": delivered_at,
        "delivery_status": delivery_status,
        "no_notify_reason": no_notify_reason,
    });
    if let Some(s) = summary {
        val["summary"] = serde_json::Value::String(s.to_owned());
    }
    // Parse notify_json into a structured object so the agent sees content directly.
    if let Some(nj) = notify_json {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(nj) {
            val["notify"] = parsed;
        }
    }
    val
}
```

- [ ] **Step 4: Update SQL queries and call sites in `memory_server.rs`**

In `crates/rightclaw-cli/src/memory_server.rs`, update the two SQL queries in `cron_list_runs` (line 215) and `cron_show_run` (line 254).

Both queries change from:

```sql
SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json
```

to:

```sql
SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at, delivery_status, no_notify_reason
```

And both `cron_run_to_json(...)` call sites need three additional row extractions appended:

```rust
row.get::<_, Option<String>>(9)?.as_deref(),
row.get::<_, Option<String>>(10)?.as_deref(),
row.get::<_, Option<String>>(11)?.as_deref(),
```

- [ ] **Step 5: Update SQL queries and call sites in `right_backend.rs`**

In `crates/rightclaw-cli/src/right_backend.rs`, make the same changes to `call_cron_list_runs` (line 342) and `call_cron_show_run` (line 377):

Both SQL queries change from:

```sql
SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json
```

to:

```sql
SELECT id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at, delivery_status, no_notify_reason
```

And both `cron_run_to_json(...)` call sites need three additional row extractions appended:

```rust
row.get::<_, Option<String>>(9)?.as_deref(),
row.get::<_, Option<String>>(10)?.as_deref(),
row.get::<_, Option<String>>(11)?.as_deref(),
```

- [ ] **Step 6: Run test to verify it passes**

Run: `devenv shell -- cargo test -p rightclaw-cli test_cron_list_runs_includes_diagnostics -- --nocapture 2>&1 | tail -20`
Expected: PASS

- [ ] **Step 7: Run all cron MCP tests**

Run: `devenv shell -- cargo test -p rightclaw-cli cron -- --nocapture 2>&1 | tail -30`
Expected: All existing and new tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw-cli/src/memory_server.rs crates/rightclaw-cli/src/right_backend.rs
git commit -m "feat(cron): expose delivered_at, delivery_status, no_notify_reason in MCP output"
```

---

### Task 6: Update skill documentation

**Files:**
- Modify: `skills/rightcron/SKILL.md:108-116`

- [ ] **Step 1: Update run record field list**

In `skills/rightcron/SKILL.md`, replace the field list at line 116:

```
Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`
```

with:

```
Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`, `summary`, `notify`, `delivered_at`, `delivery_status`, `no_notify_reason`

**Delivery diagnostics:**
- `delivery_status`: lifecycle state — `silent` (CC decided nothing to report), `pending` (awaiting delivery), `delivered` (sent to Telegram), `superseded` (newer run replaced this one), `failed` (delivery gave up after retries)
- `no_notify_reason`: CC's explanation when `notify` is null (e.g. "No changes since last run")
- `delivered_at`: timestamp when the result was delivered, or null
```

- [ ] **Step 2: Add diagnostic debugging example**

In `skills/rightcron/SKILL.md`, after the existing debugging example section (line 144), add:

```markdown

### Diagnosing missing notifications

When the user asks "why wasn't I notified?", check `delivery_status` and `no_notify_reason`:

```
1. mcp__right__cron_list_runs(job_name="github-tracker", limit=5)
   -> Check delivery_status for each run:
      - "silent" + no_notify_reason → CC decided nothing to report, reason explains why
      - "pending" → notification waiting for chat idle (3 min threshold)
      - "superseded" → newer run replaced this one before delivery
      - "failed" → delivery failed after 3 attempts, check logs
      - "delivered" → was sent to Telegram successfully
```

Never guess at delivery issues — always check the actual `delivery_status` field.
```

- [ ] **Step 3: Commit**

```bash
git add skills/rightcron/SKILL.md
git commit -m "docs(cron): document delivery diagnostics fields in rightcron skill"
```

---

### Task 7: Full build and integration verification

**Files:** None (verification only)

- [ ] **Step 1: Full workspace build**

Run: `devenv shell -- cargo build --workspace 2>&1 | tail -20`
Expected: Build succeeds with no errors.

- [ ] **Step 2: Run all tests**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -40`
Expected: All tests pass.

- [ ] **Step 3: Verify cron-schema.json is regenerated**

The `cron-schema.json` file is written by the codegen pipeline at bot startup. Verify the constant is correct:

Run: `devenv shell -- cargo test -p rightclaw pipeline -- --nocapture 2>&1 | tail -20`
Expected: Pipeline tests pass (they write and verify cron-schema.json).
