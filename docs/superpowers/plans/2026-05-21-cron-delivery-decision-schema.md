# Cron Delivery Decision Schema Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every cron result choose an explicit `delivery` decision, preserve explicit silent runs, and remove the misleading `summary`/`notify_json` async-run storage contract.

**Architecture:** `async_runs` keeps one queue/history table, but stores `run_note` and `delivery_json` instead of `summary`, `notify_json`, and `no_notify_reason`. Cron and background continuation outputs use the same parsed delivery-decision model; background keeps its existing behavior by allowing only the `notify` branch. The delivery loop queues only `delivery_required = 1` rows whose `delivery_json` parses as `kind = "notify"`.

**Tech Stack:** Rust 2024, SQLite via `rusqlite`, `rusqlite_migration`, Claude Code JSON schemas, Tokio async delivery, existing `devenv shell -- cargo ...` workflow.

**Spec:** `docs/superpowers/specs/2026-05-21-cron-delivery-decision-schema-design.md`.

---

## Scope Check

This is one coupled contract change: schema, parser, DB storage, delivery queue, and prompt docs must move together. Splitting it would leave intermediate states where cron output validates but cannot be delivered, or delivery reads renamed columns before producers write them.

Repository instruction requires `rust-dev:rust-dev` before writing Rust. In this planning session that skill was not available in the skill list and `fd -a rust-dev /Users/developer/.codex/skills` plus `fd -a rust-dev /Users/developer/.codex/plugins/cache` returned no files. At implementation start, check again and record the result before editing Rust.

If implementation uses subagents, do not use Haiku models for subagents. Use the repo's required Rust review subagent only if the `rust-dev` skill pack becomes available; otherwise record that it is unavailable and rely on compiler, tests, and local review.

## File Structure

- Modify `crates/right-db/src/sql/v23_async_runs.sql`: canonical fresh schema for new DBs.
- Create `crates/right-db/src/sql/v25_async_runs_delivery_decision.sql`: SQL-only lossy migration for existing DBs.
- Modify `crates/right-db/src/migrations.rs`: register v25 and update migration tests.
- Modify `crates/right-agent/src/async_runs.rs`: rename helper fields and JSON history output to `run_note`/`delivery_json`.
- Modify `crates/right-codegen/src/agent_def.rs`: update `CRON_SCHEMA_JSON` and `BG_CONTINUATION_SCHEMA_JSON`.
- Modify `crates/right-codegen/src/agent_def_tests.rs`: schema assertions for required `delivery`/`run_note`.
- Modify `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`: update agent-facing cron contract.
- Modify `PROMPT_SYSTEM.md`: mirror actual schema and prompt wording.
- Modify `crates/bot/src/cron.rs`: replace optional notify parser with delivery decision enum and validation.
- Modify `crates/bot/src/background.rs`: persist background outputs as notify delivery decisions and write `run_note`.
- Modify `crates/bot/src/async_delivery.rs`: query/read `delivery_json`, parse notify branch, never synthesize from `run_note`.
- Modify `crates/bot/src/telegram/worker.rs`: update background marker query from `notify_json` to `delivery_json`.
- Modify `docs/architecture/sessions.md`: cite-on-touch update for async run delivery fields.

## Task 0: Baseline And Prerequisites

**Files:**
- Read: `AGENTS.md`
- Read: `AGENTS.rust.md`
- Read: `docs/superpowers/specs/2026-05-21-cron-delivery-decision-schema-design.md`
- No code changes.

- [ ] **Step 1: Confirm clean worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: no output. If there are unrelated user changes, record them and do not include them in commits for this plan.

- [ ] **Step 2: Check for the repository-required Rust skill**

Run:

```bash
devenv shell -- fd -a rust-dev /Users/developer/.codex/skills
devenv shell -- fd -a rust-dev /Users/developer/.codex/plugins/cache
```

Expected in the current environment: no output. If the skill appears, load `rust-dev:rust-dev` before editing Rust. If it is still absent, add an implementation note in the first commit message body: `rust-dev skill unavailable in this session`.

- [ ] **Step 3: Run targeted baseline checks**

Run:

```bash
devenv shell -- cargo test -p right-db migrations
devenv shell -- cargo test -p right-agent async_runs
devenv shell -- cargo test -p right-codegen --lib
devenv shell -- cargo test -p right-bot cron::tests::parse_cron_output
devenv shell -- cargo test -p right-bot async_delivery::tests
```

Expected: all pass before behavior changes. If a command fails before edits, record the exact failing test names and continue only if the failure is unrelated to this plan.

## Task 1: Rename Async Run Storage With A Lossy SQL Migration

**Files:**
- Modify: `crates/right-db/src/sql/v23_async_runs.sql`
- Create: `crates/right-db/src/sql/v25_async_runs_delivery_decision.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Modify/Test: `crates/right-agent/src/async_runs.rs`

- [ ] **Step 1: Write failing migration tests**

In `crates/right-db/src/migrations.rs`, replace `v23_async_runs_has_delivery_columns` with:

```rust
#[test]
fn async_runs_has_delivery_decision_columns() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('async_runs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    for col in ["run_note", "delivery_json", "delivery_required", "delivery_status"] {
        assert!(cols.contains(&col.to_string()), "{col} column missing");
    }
    for col in ["summary", "notify_json", "no_notify_reason"] {
        assert!(!cols.contains(&col.to_string()), "{col} column should be removed");
    }
}

#[test]
fn v25_loses_old_pending_delivery_payloads() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_version(&mut conn, 24).unwrap();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            status, started_at, finished_at, summary, notify_json,
            no_notify_reason, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (
            'run-1', 'cron', 'ping', 'run-1', -100,
            'success', '2026-05-21T10:00:00Z', '2026-05-21T10:00:05Z',
            'old summary', '{\"content\":\"old payload\"}', NULL,
            1, 'pending', '2026-05-21T10:00:00Z', '2026-05-21T10:00:05Z'
         )",
        [],
    )
    .unwrap();

    MIGRATIONS.to_latest(&mut conn).unwrap();

    let row: (Option<String>, Option<String>, i64, String) = conn
        .query_row(
            "SELECT run_note, delivery_json, delivery_required, delivery_status
             FROM async_runs WHERE id = 'run-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, (Some("old summary".into()), None, 0, "none".into()));
}
```

- [ ] **Step 2: Run migration tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-db async_runs_has_delivery_decision_columns v25_loses_old_pending_delivery_payloads
```

Expected: fail because v25 is not registered and old columns still exist.

- [ ] **Step 3: Update fresh schema**

In `crates/right-db/src/sql/v23_async_runs.sql`, replace:

```sql
    summary              TEXT,
    notify_json          TEXT,
    no_notify_reason     TEXT,
```

with:

```sql
    run_note            TEXT,
    delivery_json       TEXT,
```

- [ ] **Step 4: Add SQL-only lossy migration**

Create `crates/right-db/src/sql/v25_async_runs_delivery_decision.sql`:

```sql
ALTER TABLE async_runs RENAME TO async_runs_old;

CREATE TABLE async_runs (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL,
    producer_ref         TEXT,
    source_session_id    TEXT,
    run_session_id       TEXT NOT NULL,
    target_chat_id       INTEGER NOT NULL,
    target_thread_id     INTEGER,
    status              TEXT NOT NULL,
    handoff_state        TEXT,
    started_at           TEXT,
    finished_at          TEXT,
    exit_code            INTEGER,
    log_path             TEXT,
    run_note            TEXT,
    delivery_json       TEXT,
    error_json           TEXT,
    delivery_required    INTEGER NOT NULL,
    delivery_status      TEXT NOT NULL,
    delivery_attempts    INTEGER NOT NULL DEFAULT 0,
    delivered_at         TEXT,
    last_delivery_error  TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

INSERT INTO async_runs (
    id, kind, producer_ref, source_session_id, run_session_id,
    target_chat_id, target_thread_id, status, handoff_state,
    started_at, finished_at, exit_code, log_path, run_note,
    delivery_json, error_json, delivery_required, delivery_status,
    delivery_attempts, delivered_at, last_delivery_error, created_at, updated_at
)
SELECT
    id, kind, producer_ref, source_session_id, run_session_id,
    target_chat_id, target_thread_id, status, handoff_state,
    started_at, finished_at, exit_code, log_path, summary,
    NULL, error_json, 0,
    CASE
      WHEN delivery_status IN ('delivered', 'superseded', 'failed') THEN delivery_status
      ELSE 'none'
    END,
    delivery_attempts, delivered_at, last_delivery_error, created_at, updated_at
FROM async_runs_old;

DROP TABLE async_runs_old;

CREATE INDEX IF NOT EXISTS idx_async_runs_kind_producer_started
    ON async_runs(kind, producer_ref, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_async_runs_delivery
    ON async_runs(delivery_required, delivery_status, status, finished_at);

CREATE INDEX IF NOT EXISTS idx_async_runs_target_status
    ON async_runs(target_chat_id, target_thread_id, status);

CREATE INDEX IF NOT EXISTS idx_async_runs_run_session
    ON async_runs(run_session_id);
```

- [ ] **Step 5: Register v25**

In `crates/right-db/src/migrations.rs`, add:

```rust
const V25_SCHEMA: &str = include_str!("sql/v25_async_runs_delivery_decision.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 25;
```

and append `M::up(V25_SCHEMA),` after the existing v24 migration in `MIGRATIONS`.

- [ ] **Step 6: Rename async run helper fields**

In `crates/right-agent/src/async_runs.rs`, change `RunOutput` to:

```rust
#[derive(Debug, Clone, Copy)]
pub struct RunOutput<'a> {
    pub run_note: Option<&'a str>,
    pub delivery_json: Option<&'a str>,
    pub error_json: Option<&'a str>,
    pub delivery_required: bool,
}
```

Change `CronRunJsonRow` fields from `summary`, `notify_json`, `no_notify_reason` to:

```rust
    pub run_note: Option<String>,
    pub delivery_json: Option<String>,
```

Update `persist_run_output` so the guard and SQL use `delivery_json`:

```rust
if output.delivery_required && output.delivery_json.is_none() {
    return Err(rusqlite::Error::InvalidParameterName(
        "delivery_json is required when delivery_required is true".into(),
    ));
}

let rows = conn.execute(
    "UPDATE async_runs
     SET run_note = ?2,
         delivery_json = ?3,
         error_json = ?4,
         delivery_required = ?5,
         delivery_status = ?6,
         updated_at = ?7
     WHERE id = ?1",
    params![
        run_id,
        output.run_note,
        output.delivery_json,
        output.error_json,
        output.delivery_required,
        delivery_status,
        now,
    ],
)?;
```

Update `cron_run_to_json` to emit `run_note` and `delivery`:

```rust
if let Some(run_note) = &row.run_note {
    val["run_note"] = serde_json::Value::String(run_note.clone());
}

if let Some(delivery_json) = &row.delivery_json
    && let Ok(delivery) = serde_json::from_str::<serde_json::Value>(delivery_json)
{
    val["delivery"] = delivery;
}
```

- [ ] **Step 7: Update helper tests**

In `crates/right-agent/src/async_runs.rs`, update test assertions and inserts to use `run_note`/`delivery_json`. Add this test:

```rust
#[test]
fn persist_run_output_requires_delivery_json_when_delivery_required() {
    let conn = setup();
    insert_running_cron_run(
        &conn,
        NewCronRun {
            id: "run-1",
            job_name: "job-a",
            started_at: "2026-05-18T10:00:00Z",
            log_path: "/log/run-1.ndjson",
            target_chat_id: Some(-100),
            target_thread_id: None,
        },
    )
    .unwrap();

    let err = persist_run_output(
        &conn,
        "run-1",
        RunOutput {
            run_note: Some("note"),
            delivery_json: None,
            error_json: None,
            delivery_required: true,
        },
    )
    .expect_err("delivery_required without delivery_json should fail");

    assert!(matches!(
        err,
        rusqlite::Error::InvalidParameterName(ref name)
            if name == "delivery_json is required when delivery_required is true"
    ));
}
```

- [ ] **Step 8: Run targeted DB/helper tests**

Run:

```bash
devenv shell -- cargo test -p right-db async_runs
devenv shell -- cargo test -p right-agent async_runs
```

Expected: pass.

- [ ] **Step 9: Commit**

```bash
git add crates/right-db/src/sql/v23_async_runs.sql \
        crates/right-db/src/sql/v25_async_runs_delivery_decision.sql \
        crates/right-db/src/migrations.rs \
        crates/right-agent/src/async_runs.rs
git commit -m "refactor(async-runs): rename delivery result storage"
```

## Task 2: Update JSON Schemas And Cron Instructions

**Files:**
- Modify: `crates/right-codegen/src/agent_def.rs`
- Modify: `crates/right-codegen/src/agent_def_tests.rs`
- Modify: `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Write failing schema tests**

In `crates/right-codegen/src/agent_def_tests.rs`, replace cron/BG summary-notify tests with:

```rust
#[test]
fn cron_schema_requires_delivery_and_run_note() {
    let v: serde_json::Value = serde_json::from_str(CRON_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"delivery"), "delivery must be required");
    assert!(names.contains(&"run_note"), "run_note must be required");
}

#[test]
fn cron_schema_has_notify_and_silent_delivery_branches() {
    let v: serde_json::Value = serde_json::from_str(CRON_SCHEMA_JSON).unwrap();
    let branches = v["properties"]["delivery"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 2);
    let kinds: Vec<&str> = branches
        .iter()
        .filter_map(|b| b["properties"]["kind"]["const"].as_str())
        .collect();
    assert!(kinds.contains(&"notify"));
    assert!(kinds.contains(&"silent"));
}

#[test]
fn bg_continuation_schema_requires_notify_delivery_and_run_note() {
    let v: serde_json::Value = serde_json::from_str(BG_CONTINUATION_SCHEMA_JSON).unwrap();
    let required = v["required"].as_array().unwrap();
    let names: Vec<&str> = required.iter().filter_map(|x| x.as_str()).collect();
    assert!(names.contains(&"delivery"), "delivery must be required");
    assert!(names.contains(&"run_note"), "run_note must be required");

    let kind = v["properties"]["delivery"]["properties"]["kind"]["const"]
        .as_str()
        .unwrap();
    assert_eq!(kind, "notify");
}

#[test]
fn schemas_do_not_use_old_cron_output_names() {
    for schema in [CRON_SCHEMA_JSON, BG_CONTINUATION_SCHEMA_JSON] {
        let v: serde_json::Value = serde_json::from_str(schema).unwrap();
        let props = v["properties"].as_object().unwrap();
        assert!(!props.contains_key("summary"));
        assert!(!props.contains_key("notify"));
        assert!(!props.contains_key("no_notify_reason"));
    }
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-codegen cron_schema bg_continuation_schema schemas_do_not_use_old_cron_output_names
```

Expected: fail because schemas still use `summary`/`notify`.

- [ ] **Step 3: Replace `CRON_SCHEMA_JSON`**

In `crates/right-codegen/src/agent_def.rs`, replace `CRON_SCHEMA_JSON` with a compact JSON string equivalent to the spec's schema:

```rust
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"delivery":{"oneOf":[{"type":"object","properties":{"kind":{"const":"notify"},"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},{"type":"object","properties":{"kind":{"const":"silent"},"reason":{"type":"string","minLength":1}},"required":["kind","reason"]}]},"run_note":{"type":"string"}},"required":["delivery","run_note"]}"#;
```

- [ ] **Step 4: Replace `BG_CONTINUATION_SCHEMA_JSON`**

Use the same field names, but allow only notify delivery:

```rust
pub const BG_CONTINUATION_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"delivery":{"type":"object","properties":{"kind":{"const":"notify"},"content":{"type":"string","minLength":1},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]},"media_group_id":{"type":["string","null"]}},"required":["type","path"]}}},"required":["kind","content"]},"run_note":{"type":"string"}},"required":["delivery","run_note"]}"#;
```

- [ ] **Step 5: Update cron instructions**

In `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`, replace mentions of `notify.content`, `notify: null`, `no_notify_reason`, and `summary` with:

```markdown
- `delivery.kind = "notify"` with non-empty `delivery.content` -> message delivered.
- `delivery.kind = "silent"` -> no Telegram message. Use it only when the task is conditional and there is factually nothing to report. Put the factual reason in `delivery.reason`.

`run_note` is technical metadata for logs and run history. It is not delivered to Telegram.

For reminders, pings, tags, tell/message requests, and explicit notification tasks, choose `delivery.kind = "notify"` and put the complete user-facing Telegram text in `delivery.content`.
```

Keep the existing "No clarifying questions" section, but update examples to reference `delivery.content` and `delivery.reason`.

- [ ] **Step 6: Update `PROMPT_SYSTEM.md`**

In `PROMPT_SYSTEM.md`, update the schema section so it states:

```markdown
### CRON_SCHEMA_JSON (cron jobs)

Required: `delivery` and `run_note`.

`delivery` is exactly one of:

- `{"kind":"notify","content":"...","attachments":null}` - user-facing Telegram delivery.
- `{"kind":"silent","reason":"..."}` - explicit silent run for conditional checks with nothing to report.

`run_note` is technical history/debug metadata and is never delivered.

### BG_CONTINUATION_SCHEMA_JSON (Telegram background continuation)

Required: `delivery` and `run_note`. `delivery.kind` is always `"notify"` and `delivery.content` has `minLength: 1`; silent output is forbidden.
```

- [ ] **Step 7: Run schema/prompt tests**

Run:

```bash
devenv shell -- cargo test -p right-codegen --lib
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add crates/right-codegen/src/agent_def.rs \
        crates/right-codegen/src/agent_def_tests.rs \
        crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md \
        PROMPT_SYSTEM.md
git commit -m "feat(cron): require explicit delivery decision output"
```

## Task 3: Parse And Validate Delivery Decisions

**Files:**
- Modify/Test: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/background.rs`

- [ ] **Step 1: Write failing parser tests**

In `crates/bot/src/cron.rs`, replace old `parse_cron_output_*` tests with:

```rust
#[test]
fn parse_cron_output_notify_delivery() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":"BTC broke 100k","attachments":null},"run_note":"Checked 5 pairs"}}"#.to_string(),
    ];
    let out = parse_cron_output(&lines).unwrap();
    assert_eq!(out.run_note, "Checked 5 pairs");
    let notify = out.delivery.as_notify().unwrap();
    assert_eq!(notify.content, "BTC broke 100k");
    assert!(notify.attachments.is_none());
}

#[test]
fn parse_cron_output_silent_delivery() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"silent","reason":"No changes since last run"},"run_note":"Checked feed"}}"#.to_string(),
    ];
    let out = parse_cron_output(&lines).unwrap();
    assert!(matches!(out.delivery, CronDeliveryDecision::Silent { .. }));
    assert_eq!(out.delivery.silent_reason(), Some("No changes since last run"));
}

#[test]
fn parse_cron_output_missing_delivery_is_invalid() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","structured_output":{"run_note":"note only"}}"#.to_string(),
    ];
    let err = parse_cron_output(&lines).unwrap_err();
    assert!(err.contains("delivery"));
}

#[test]
fn parse_cron_output_empty_notify_content_is_invalid() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"notify","content":"   "},"run_note":"bad"}}"#.to_string(),
    ];
    let err = parse_cron_output(&lines).unwrap_err();
    assert!(err.contains("empty notify content"));
}

#[test]
fn parse_cron_output_empty_silent_reason_is_invalid() {
    let lines = vec![
        r#"{"type":"result","subtype":"success","structured_output":{"delivery":{"kind":"silent","reason":" "},"run_note":"bad"}}"#.to_string(),
    ];
    let err = parse_cron_output(&lines).unwrap_err();
    assert!(err.contains("empty silent reason"));
}
```

- [ ] **Step 2: Run parser tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_cron_output
```

Expected: fail because the parser still expects `notify`/`summary`.

- [ ] **Step 3: Replace output structs**

In `crates/bot/src/cron.rs`, replace `CronReplyOutput` with:

```rust
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CronReplyOutput {
    pub delivery: CronDeliveryDecision,
    pub run_note: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CronDeliveryDecision {
    Notify {
        content: String,
        attachments: Option<Vec<OutboundAttachment>>,
    },
    Silent {
        reason: String,
    },
}

impl CronDeliveryDecision {
    pub(crate) fn as_notify(&self) -> Option<CronNotify> {
        match self {
            Self::Notify { content, attachments } => Some(CronNotify {
                content: content.clone(),
                attachments: attachments.clone(),
            }),
            Self::Silent { .. } => None,
        }
    }

    pub(crate) fn silent_reason(&self) -> Option<&str> {
        match self {
            Self::Silent { reason } => Some(reason),
            Self::Notify { .. } => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Notify { content, .. } if content.trim().is_empty() => {
                Err("empty notify content".to_string())
            }
            Self::Silent { reason } if reason.trim().is_empty() => {
                Err("empty silent reason".to_string())
            }
            _ => Ok(()),
        }
    }
}
```

Add helper for delivery loop/background failure payloads:

```rust
pub(crate) fn notify_delivery_json(notify: &CronNotify) -> Result<String, serde_json::Error> {
    serde_json::to_string(&CronDeliveryDecision::Notify {
        content: notify.content.clone(),
        attachments: notify.attachments.clone(),
    })
}

pub(crate) fn notify_from_delivery_json(raw: &str) -> Result<CronNotify, String> {
    let decision: CronDeliveryDecision =
        serde_json::from_str(raw).map_err(|e| format!("parse delivery_json: {e}"))?;
    decision
        .as_notify()
        .ok_or_else(|| "delivery_json is not a notify decision".to_string())
}
```

- [ ] **Step 4: Validate after serde parse**

In `parse_cron_output`, after `serde_json::from_value`, validate:

```rust
let output: CronReplyOutput = serde_json::from_value(payload.clone())
    .map_err(|e| format!("failed to parse CronReplyOutput: {e}"))?;
output.delivery.validate()?;
Ok(output)
```

- [ ] **Step 5: Update background success path**

In `crates/bot/src/background.rs`, replace `output.notify` access with:

```rust
let notify = output
    .delivery
    .as_notify()
    .ok_or_else(|| "background continuation returned non-notify delivery".to_string())?;
```

Persist with:

```rust
right_agent::async_runs::RunOutput {
    run_note: Some(&output.run_note),
    delivery_json: Some(&delivery_json),
    error_json: None,
    delivery_required: true,
}
```

where `delivery_json` is produced by serializing a notify delivery decision after attachment paths are converted to host paths.

- [ ] **Step 6: Run parser/background tests**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_cron_output
devenv shell -- cargo test -p right-bot background
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/background.rs
git commit -m "refactor(bot): parse cron delivery decisions"
```

## Task 4: Persist Cron Decisions And Update Delivery Loop

**Files:**
- Modify/Test: `crates/bot/src/cron.rs`
- Modify/Test: `crates/bot/src/async_delivery.rs`
- Modify/Test: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Write failing delivery tests**

In `crates/bot/src/async_delivery.rs`, update test helpers to insert `run_note` and `delivery_json`. Add:

```rust
#[test]
fn fetch_pending_ignores_silent_delivery_decision() {
    let (_dir, conn) = setup_db();
    insert_async_cron_run(
        &conn,
        TestCronRun {
            id: "silent",
            delivery_json: Some(r#"{"kind":"silent","reason":"No changes"}"#),
            delivery_required: Some(false),
            delivery_status: Some("none"),
            ..Default::default()
        },
    );
    assert!(fetch_pending(&conn).unwrap().is_none());
}

#[test]
fn format_async_yaml_rejects_silent_delivery_json() {
    let pending = PendingAsyncResult {
        id: "run-1".into(),
        kind: "cron".into(),
        producer_ref: Some("job".into()),
        delivery_json: r#"{"kind":"silent","reason":"No changes"}"#.into(),
        run_note: "checked".into(),
        status: "success".into(),
        target_chat_id: Some(-100),
        target_thread_id: None,
    };
    let err = format_async_yaml(&pending, 0).unwrap_err();
    assert!(err.contains("not a notify decision"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot async_delivery
```

Expected: fail because code still names `notify_json`/`summary` and parses `CronNotify` directly.

- [ ] **Step 3: Update cron persistence**

In `crates/bot/src/cron.rs`, change `persist_successful_cron_output` to derive queue state from `CronDeliveryDecision`:

```rust
fn persist_successful_cron_output(
    conn: &rusqlite::Connection,
    run_id: &str,
    cron_output: &CronReplyOutput,
    delivery_json: &str,
) -> Result<&'static str, rusqlite::Error> {
    let delivery_required = matches!(cron_output.delivery, CronDeliveryDecision::Notify { .. });
    let delivery_status = if delivery_required { "pending" } else { "none" };
    right_agent::async_runs::persist_run_output(
        conn,
        run_id,
        right_agent::async_runs::RunOutput {
            run_note: Some(&cron_output.run_note),
            delivery_json: Some(delivery_json),
            error_json: None,
            delivery_required,
        },
    )?;
    Ok(delivery_status)
}
```

At the parse success site, serialize:

```rust
let delivery_json = serde_json::to_string(&cron_output.delivery)
    .map_err(|e| format!("failed to serialize delivery_json: {e:#}"));
```

When attachments need host-path conversion, keep the branch-specific conversion for notify and then serialize the converted `CronDeliveryDecision::Notify`.

- [ ] **Step 4: Update delivery structs and SQL**

In `crates/bot/src/async_delivery.rs`, rename:

```rust
pub delivery_json: String,
pub run_note: String,
```

Update pending queries:

```sql
SELECT id, kind, producer_ref, delivery_json, COALESCE(run_note, ''), status,
       NULLIF(target_chat_id, 0), target_thread_id
FROM async_runs
WHERE delivery_required = 1
  AND delivery_status IN ('pending', 'retryable')
  AND status IN ('success', 'failed')
  AND delivery_json IS NOT NULL
ORDER BY finished_at ASC
LIMIT ?1
```

In `format_async_yaml`, replace direct `serde_json::from_str::<CronNotify>` with:

```rust
let notify = crate::cron::notify_from_delivery_json(&pending.delivery_json)?;
```

and output:

```rust
output.push_str(&format!(
    "    run_note: \"{}\"\n",
    crate::telegram::attachments::yaml_escape_string(&pending.run_note)
));
```

Change `format_async_yaml` return type to `Result<String, String>` and update the callsite that marks malformed payloads failed.

- [ ] **Step 5: Update background marker query**

In `crates/bot/src/telegram/worker.rs`, replace `notify_json IS NOT NULL` with `delivery_json IS NOT NULL` in `build_bg_marker_for_chat`.

- [ ] **Step 6: Run targeted delivery tests**

Run:

```bash
devenv shell -- cargo test -p right-bot async_delivery
devenv shell -- cargo test -p right-bot cron
devenv shell -- cargo test -p right-bot telegram::worker::tests::build_bg_marker
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/async_delivery.rs crates/bot/src/telegram/worker.rs
git commit -m "fix(cron): deliver only explicit notify decisions"
```

## Task 5: Update Remaining Async Run Consumers And Docs

**Files:**
- Modify: `crates/bot/src/background.rs`
- Modify: `crates/right-agent/src/cron_spec.rs`
- Modify: `crates/right-agent/src/cron_spec_tests.rs`
- Modify: `docs/architecture/sessions.md`
- Search all crates for old async-run column names.

- [ ] **Step 1: Search for remaining old async-run fields**

Run:

```bash
devenv shell -- rg -n "notify_json|no_notify_reason|\\bsummary\\b" crates/bot/src crates/right-agent/src crates/right-db/src crates/right-codegen/src crates/right-codegen/templates PROMPT_SYSTEM.md docs/architecture
```

Expected after this task: no `notify_json` or `no_notify_reason` remain in active async-run code or prompt/schema docs. `summary` may remain only for unrelated learning/usage/bootstrap concepts and historical migration comments.

- [ ] **Step 2: Update direct SQL in background failure handling**

In `crates/bot/src/background.rs`, change direct `UPDATE async_runs SET summary = ..., notify_json = ..., no_notify_reason = NULL` to:

```sql
UPDATE async_runs
SET run_note = ?2,
    delivery_json = ?3,
    error_json = ?4,
    delivery_required = 1,
    delivery_status = 'pending',
    handoff_state = 'failed',
    finished_at = ?5,
    exit_code = NULL,
    status = 'failed',
    updated_at = ?5
WHERE id = ?1
  AND kind = 'background'
  AND status = 'queued'
  AND handoff_state = 'queued'
```

Make `background_failure_payload` return a notify delivery decision JSON using `crate::cron::notify_delivery_json(&notify)`.

- [ ] **Step 3: Update cron spec target propagation tests**

In `crates/right-agent/src/cron_spec_tests.rs`, rename helper local `notify_json` to `delivery_json` and insert delivery payloads shaped as:

```json
{"kind":"notify","content":"queued"}
```

Update SQL column names from `notify_json` to `delivery_json`.

- [ ] **Step 4: Update architecture doc**

In `docs/architecture/sessions.md`, replace the reflection/cron delivery storage description with:

```markdown
Cron success output stores `async_runs.run_note` plus a structured
`delivery_json` decision. `delivery.kind = "notify"` enters the async delivery
queue; `delivery.kind = "silent"` is a completed non-delivering run. The
delivery loop never uses `run_note` as fallback Telegram content.
```

Also update the reflection failure bullet so it says reflection reply is stored
as notify `delivery_json`, not `notify_json`.

- [ ] **Step 5: Run old-name search**

Run:

```bash
devenv shell -- rg -n "notify_json|no_notify_reason" crates/bot/src crates/right-agent/src crates/right-db/src/sql crates/right-codegen/src crates/right-codegen/templates PROMPT_SYSTEM.md docs/architecture
```

Expected: no matches except comments inside old pre-v25 migration functions that describe historical `cron_runs` migration. Those historical comments may remain because they document older migration behavior.

- [ ] **Step 6: Run targeted consumer tests**

Run:

```bash
devenv shell -- cargo test -p right-agent cron_spec
devenv shell -- cargo test -p right-bot background
devenv shell -- cargo test -p right-bot telegram::worker::tests::build_bg_marker
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/background.rs \
        crates/right-agent/src/cron_spec.rs \
        crates/right-agent/src/cron_spec_tests.rs \
        docs/architecture/sessions.md
git commit -m "refactor(async-runs): update delivery decision consumers"
```

## Task 6: Full Verification And Final Review

**Files:**
- No planned code changes unless verification finds issues.

- [ ] **Step 1: Run formatting**

Run:

```bash
devenv shell -- cargo fmt --all
```

Expected: exits 0. Commit formatting only if it touched files changed by this plan.

- [ ] **Step 2: Run targeted package checks**

Run:

```bash
devenv shell -- cargo test -p right-db
devenv shell -- cargo test -p right-agent
devenv shell -- cargo test -p right-codegen
devenv shell -- cargo test -p right-bot cron async_delivery background
```

Expected: pass.

- [ ] **Step 3: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: pass.

- [ ] **Step 4: Run final debug build**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: pass.

- [ ] **Step 5: Rust review gate**

If `rust-dev:review-rust-code` is available in the implementation session, run it against the branch. Convert each accepted finding into a concrete fix commit. If the skill is unavailable, record `rust-dev review skill unavailable` in the final implementation notes.

- [ ] **Step 6: Final search**

Run:

```bash
devenv shell -- rg -n "notify_json|no_notify_reason" crates/bot/src crates/right-agent/src crates/right-db/src/sql/v23_async_runs.sql crates/right-codegen/src crates/right-codegen/templates PROMPT_SYSTEM.md docs/architecture
```

Expected: no matches. Do not include `crates/right-db/src/migrations.rs` in this final search because historical migrations still mention old `cron_runs` columns.

- [ ] **Step 7: Commit verification/doc fallout**

If steps 1-6 changed files, commit them:

```bash
git add crates/right-db/src/sql/v23_async_runs.sql \
        crates/right-db/src/sql/v25_async_runs_delivery_decision.sql \
        crates/right-db/src/migrations.rs \
        crates/right-agent/src/async_runs.rs \
        crates/right-agent/src/cron_spec.rs \
        crates/right-agent/src/cron_spec_tests.rs \
        crates/right-codegen/src/agent_def.rs \
        crates/right-codegen/src/agent_def_tests.rs \
        crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md \
        crates/bot/src/cron.rs \
        crates/bot/src/background.rs \
        crates/bot/src/async_delivery.rs \
        crates/bot/src/telegram/worker.rs \
        PROMPT_SYSTEM.md \
        docs/architecture/sessions.md
git commit -m "test(cron): verify delivery decision contract"
```

If no files changed, do not create an empty commit.

## Acceptance Checklist

- [ ] `CRON_SCHEMA_JSON` requires `delivery` and `run_note`; missing delivery cannot validate.
- [ ] `BG_CONTINUATION_SCHEMA_JSON` uses `delivery.kind = "notify"` and `run_note`; silent background output cannot validate.
- [ ] `parse_cron_output` rejects missing delivery, empty notify content, and empty silent reason.
- [ ] Cron notify output persists `delivery_required = 1`, `delivery_status = 'pending'`, and notify `delivery_json`.
- [ ] Cron silent output persists `delivery_required = 0`, `delivery_status = 'none'`, and silent `delivery_json`.
- [ ] Async delivery reads `delivery_json` and never uses `run_note` as fallback Telegram content.
- [ ] SQL-only v25 migration renames `summary` to `run_note`, drops old payload columns, and clears old pending deliveries.
- [ ] `PROMPT_SYSTEM.md` and `CRON_INSTRUCTIONS.md` describe the actual contract.
- [ ] `docs/architecture/sessions.md` is updated for the async-run delivery fields.
- [ ] `devenv shell -- cargo test --workspace` passes.
- [ ] `devenv shell -- cargo build --workspace` passes.
