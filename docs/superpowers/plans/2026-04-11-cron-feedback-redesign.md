# Cron Feedback Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver cron results through the main CC session so users can reply to cron notifications naturally.

**Architecture:** Cron execution stays as-is (separate `claude -p`), but instead of `bot.send_message()`, results are persisted to DB with a new schema. A delivery poll loop waits for 5 min idle, then pipes results through the main session's `claude -p` (with `--resume`). Shared `Arc<AtomicI64>` idle timestamp coordinates handler/worker/delivery.

**Tech Stack:** Rust, tokio, rusqlite, serde_json, chrono, teloxide

---

### Task 1: Add `CRON_SCHEMA_JSON` constant

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:16-20`

- [ ] **Step 1: Add the new constant below `BOOTSTRAP_SCHEMA_JSON`**

In `crates/rightclaw/src/codegen/agent_def.rs`, add after line 26:

```rust
/// JSON schema for cron job structured output.
///
/// `summary` is always required. `notify` is null when the cron ran silently
/// (no user notification needed). When `notify` is present, `content` is required.
pub const CRON_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"notify":{"type":["object","null"],"properties":{"content":{"type":"string"},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]},"summary":{"type":"string"}},"required":["summary"]}"#;
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p rightclaw`

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs
git commit -m "feat(cron): add CRON_SCHEMA_JSON constant for cron structured output"
```

---

### Task 2: DB migration — extend `cron_runs` table

**Files:**
- Create: `crates/rightclaw/src/memory/sql/v5_cron_feedback.sql`
- Modify: `crates/rightclaw/src/memory/migrations.rs`

- [ ] **Step 1: Write the failing migration test**

In `crates/rightclaw/src/memory/migrations.rs`, add a test after the existing tests:

```rust
#[test]
fn migrations_apply_cleanly_to_v5() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();
    // Verify new columns exist on cron_runs
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('cron_runs')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(cols.contains(&"summary".to_string()), "summary column missing");
    assert!(cols.contains(&"notify_json".to_string()), "notify_json column missing");
    assert!(cols.contains(&"delivered_at".to_string()), "delivered_at column missing");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw migrations_apply_cleanly_to_v5`
Expected: FAIL — V5 migration doesn't exist yet.

- [ ] **Step 3: Create the SQL migration file**

Create `crates/rightclaw/src/memory/sql/v5_cron_feedback.sql`:

```sql
-- V5: Extend cron_runs with feedback columns for delivery-through-main-session.
-- summary: always written on successful cron completion.
-- notify_json: serialized notify object (content + attachments) or NULL if silent.
-- delivered_at: set when result is delivered through main CC session.
ALTER TABLE cron_runs ADD COLUMN summary TEXT;
ALTER TABLE cron_runs ADD COLUMN notify_json TEXT;
ALTER TABLE cron_runs ADD COLUMN delivered_at TEXT;
```

- [ ] **Step 4: Wire the migration**

In `crates/rightclaw/src/memory/migrations.rs`, add:

```rust
const V5_SCHEMA: &str = include_str!("sql/v5_cron_feedback.sql");
```

And update the `MIGRATIONS` vec to include `M::up(V5_SCHEMA)`.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rightclaw migrations`
Expected: All migration tests pass, including the new `migrations_apply_cleanly_to_v5`.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/memory/sql/v5_cron_feedback.sql crates/rightclaw/src/memory/migrations.rs
git commit -m "feat(cron): V5 migration — add summary, notify_json, delivered_at to cron_runs"
```

---

### Task 3: New `CronReplyOutput` struct and parser

**Files:**
- Modify: `crates/bot/src/cron.rs`

This task replaces the old `parse_cron_reply_content` (which returned `Option<String>`) with a new parser that returns the full cron output including notify and summary.

- [ ] **Step 1: Write failing tests for the new parser**

In `crates/bot/src/cron.rs`, replace the existing `parse_cron_reply_content_*` tests (lines 632-672) with:

```rust
// -- CronReplyOutput parser tests --

#[test]
fn parse_cron_output_full_notify() {
    let json = r#"{"result":{"notify":{"content":"BTC broke 100k","attachments":null},"summary":"Checked 5 pairs"}}"#;
    let out = parse_cron_output(json.as_bytes()).unwrap();
    assert_eq!(out.summary, "Checked 5 pairs");
    let notify = out.notify.unwrap();
    assert_eq!(notify.content, "BTC broke 100k");
    assert!(notify.attachments.is_none());
}

#[test]
fn parse_cron_output_silent_null_notify() {
    let json = r#"{"result":{"notify":null,"summary":"Nothing interesting"}}"#;
    let out = parse_cron_output(json.as_bytes()).unwrap();
    assert!(out.notify.is_none());
    assert_eq!(out.summary, "Nothing interesting");
}

#[test]
fn parse_cron_output_with_attachments() {
    let json = r#"{"result":{"notify":{"content":"Chart","attachments":[{"type":"photo","path":"/sandbox/outbox/chart.png"}]},"summary":"Generated chart"}}"#;
    let out = parse_cron_output(json.as_bytes()).unwrap();
    let notify = out.notify.unwrap();
    assert_eq!(notify.attachments.as_ref().unwrap().len(), 1);
    assert_eq!(notify.attachments.unwrap()[0].path, "/sandbox/outbox/chart.png");
}

#[test]
fn parse_cron_output_structured_output_preferred() {
    let json = r#"{"result":"ignored","structured_output":{"notify":null,"summary":"from structured"}}"#;
    let out = parse_cron_output(json.as_bytes()).unwrap();
    assert_eq!(out.summary, "from structured");
}

#[test]
fn parse_cron_output_unparseable_returns_err() {
    let result = parse_cron_output(b"not json");
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot parse_cron_output`
Expected: FAIL — `parse_cron_output` doesn't exist, `CronReplyOutput` doesn't exist.

- [ ] **Step 3: Add the structs and parser**

In `crates/bot/src/cron.rs`, add the new types near the top (after `CronError`):

```rust
/// Structured output from a cron CC invocation.
#[derive(Debug, serde::Deserialize)]
pub struct CronReplyOutput {
    pub notify: Option<CronNotify>,
    pub summary: String,
}

/// User-facing notification from a cron job.
#[derive(Debug, serde::Deserialize)]
pub struct CronNotify {
    pub content: String,
    pub attachments: Option<Vec<crate::telegram::attachments::OutboundAttachment>>,
}
```

Replace `parse_cron_reply_content` (lines 344-356) with:

```rust
/// Parse CC stdout into `CronReplyOutput`.
///
/// Tries `structured_output` first, falls back to `result`.
/// Returns `Err` if neither field is present or JSON is invalid.
pub(crate) fn parse_cron_output(stdout: &[u8]) -> Result<CronReplyOutput, String> {
    let raw = String::from_utf8_lossy(stdout);

    // Parse the outer CC JSON envelope
    let envelope: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("CC output is not valid JSON: {e}"))?;

    // Prefer structured_output over result (CC behavior varies after MCP tool use)
    let payload = if let Some(so) = envelope.get("structured_output") {
        if !so.is_null() { so } else { envelope.get("result").unwrap_or(so) }
    } else if let Some(r) = envelope.get("result") {
        r
    } else {
        return Err("CC output has neither 'structured_output' nor 'result' field".into());
    };

    serde_json::from_value(payload.clone())
        .map_err(|e| format!("failed to parse CronReplyOutput: {e}"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot parse_cron_output`
Expected: All 5 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): CronReplyOutput struct and parse_cron_output parser"
```

---

### Task 4: Refactor `execute_job` — persist to DB instead of `bot.send_message`

**Files:**
- Modify: `crates/bot/src/cron.rs`

This task changes `execute_job` to:
1. Use `CRON_SCHEMA_JSON` instead of `REPLY_SCHEMA_JSON`
2. Parse with `parse_cron_output` instead of `parse_cron_reply_content`
3. Persist `summary` + `notify_json` to DB instead of sending to Telegram
4. Download attachments from sandbox to `outbox/cron/{run_id}/`

- [ ] **Step 1: Remove `bot` and `notify_chat_ids` from `execute_job` signature**

Update `execute_job` signature (line 145) — remove `bot: &BotType` and `notify_chat_ids: &[i64]` params. Add `ssh_config_path: Option<&std::path::Path>` and `agent_name_for_sandbox: &str` (needed for attachment download).

New signature:

```rust
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
)
```

- [ ] **Step 2: Write cron-schema.json instead of reply-schema.json**

Replace the reply schema reading block (lines 218-228) with writing the cron schema:

```rust
// Write cron schema for this invocation
let cron_schema_path = agent_dir.join(".claude").join("cron-schema.json");
if let Err(e) = std::fs::write(&cron_schema_path, rightclaw::codegen::CRON_SCHEMA_JSON) {
    tracing::error!(job = %job_name, "failed to write cron-schema.json: {e:#}");
    update_run_record(&conn, &run_id, None, "failed");
    std::fs::remove_file(&lock_path).ok();
    return;
}
```

Update the `--json-schema` arg (line 243-245) to use the schema content directly:

```rust
cmd.arg("--json-schema").arg(rightclaw::codegen::CRON_SCHEMA_JSON);
```

Remove the old `reply_schema` variable and its `is_some()` checks.

- [ ] **Step 3: Replace Telegram delivery with DB persist + attachment download**

Replace lines 313-335 (the CRON-reply block) with:

```rust
// Parse cron output and persist to DB
if output.status.success() {
    match parse_cron_output(&output.stdout) {
        Ok(cron_output) => {
            // Download attachments from sandbox to host outbox before persisting paths
            let notify_json = if let Some(ref notify) = cron_output.notify {
                // Download attachments from sandbox if present
                if let Some(ref atts) = notify.attachments {
                    let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                    if let Err(e) = std::fs::create_dir_all(&outbox_dir) {
                        tracing::error!(job = %job_name, "failed to create cron outbox dir: {e:#}");
                    } else if ssh_config_path.is_some() {
                        let sandbox = rightclaw::openshell::sandbox_name(agent_name);
                        for att in atts {
                            let file_name = std::path::Path::new(&att.path)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            let dest = outbox_dir.join(&file_name);
                            if let Err(e) = rightclaw::openshell::download_file(
                                &sandbox, &att.path, &dest,
                            ).await {
                                tracing::error!(
                                    job = %job_name,
                                    path = %att.path,
                                    "failed to download cron attachment: {e:#}"
                                );
                            }
                        }
                    }

                    // Rewrite paths to host-side for notify_json
                    let outbox_dir = agent_dir.join("outbox").join("cron").join(&run_id);
                    let host_notify = CronNotify {
                        content: notify.content.clone(),
                        attachments: notify.attachments.as_ref().map(|atts| {
                            atts.iter().map(|att| {
                                let file_name = std::path::Path::new(&att.path)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned();
                                crate::telegram::attachments::OutboundAttachment {
                                    r#type: att.r#type.clone(),
                                    path: outbox_dir.join(&file_name).to_string_lossy().into_owned(),
                                    filename: att.filename.clone(),
                                    caption: att.caption.clone(),
                                }
                            }).collect()
                        }),
                    };
                    serde_json::to_string(&host_notify).ok()
                } else {
                    // No attachments — serialize notify as-is
                    serde_json::to_string(notify).ok()
                }
            } else {
                None
            };

            // Persist summary and notify_json to DB
            if let Err(e) = conn.execute(
                "UPDATE cron_runs SET summary = ?1, notify_json = ?2 WHERE id = ?3",
                rusqlite::params![cron_output.summary, notify_json, run_id],
            ) {
                tracing::error!(job = %job_name, "failed to persist cron output to DB: {e:#}");
            }

            tracing::info!(
                job = %job_name,
                has_notify = cron_output.notify.is_some(),
                "cron output persisted to DB"
            );
        }
        Err(reason) => {
            tracing::warn!(job = %job_name, reason, "failed to parse cron output");
        }
    }
}
```

- [ ] **Step 4: Add `Serialize` derive to `CronNotify`**

Update the `CronNotify` struct to also derive `Serialize`:

```rust
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct CronNotify {
    pub content: String,
    pub attachments: Option<Vec<crate::telegram::attachments::OutboundAttachment>>,
}
```

Also ensure `OutboundAttachment` derives `Serialize` and `Clone` — check `crates/bot/src/telegram/attachments.rs` for the struct definition and add if missing.

- [ ] **Step 5: Update `run_job_loop` and `reconcile_jobs` to remove bot/chat_ids params**

Update `run_job_loop` signature — remove `bot: BotType` and `notify_chat_ids: Vec<i64>`, add `ssh_config_path: Option<PathBuf>`:

```rust
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
)
```

Update the `execute_job` call inside `run_job_loop` (line 508-509):

```rust
tokio::spawn(async move {
    execute_job(&jn, &sp, &ad, &an, md.as_deref(), scp.as_deref()).await;
});
```

Update `reconcile_jobs` signature similarly — remove `bot` and `notify_chat_ids`, add `ssh_config_path`.

- [ ] **Step 6: Update `run_cron_task` signature**

Replace `bot: BotType` and `notify_chat_ids: Vec<i64>` with `ssh_config_path: Option<PathBuf>`:

```rust
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    shutdown: CancellationToken,
)
```

Remove the `use teloxide::prelude::Requester as _;` import and the `use crate::telegram::{worker::parse_reply_output, BotType};` import (replace with just what's needed).

- [ ] **Step 7: Update `lib.rs` cron spawn site**

In `crates/bot/src/lib.rs` (lines 186-196), update the cron spawn to pass `ssh_config_path` instead of `bot` and `chat_ids`:

```rust
let cron_agent_dir = agent_dir.clone();
let cron_agent_name = args.agent.clone();
let cron_model = config.model.clone();
let cron_ssh_config = ssh_config_path.clone();
let cron_shutdown = shutdown.clone();
let cron_handle = tokio::spawn(async move {
    cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_model, cron_ssh_config, cron_shutdown).await;
});
```

Remove the `cron_bot`, `cron_chat_ids` variables.

- [ ] **Step 8: Verify it compiles**

Run: `cargo check -p rightclaw-bot`

- [ ] **Step 9: Run existing cron tests**

Run: `cargo test -p rightclaw-bot cron`
Expected: All pass (the pure-logic tests like lock/spec parsing are unaffected; the removed `parse_cron_reply_content` tests were replaced in Task 3).

- [ ] **Step 10: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "feat(cron): persist results to DB instead of direct Telegram delivery"
```

---

### Task 5: Shared idle timestamp

**Files:**
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`

This task adds a shared `Arc<AtomicI64>` that stores the unix timestamp (seconds) of the last interaction. Both handler (on incoming message) and worker (after reply sent) update it.

- [ ] **Step 1: Add `IdleTimestamp` newtype to handler.rs**

In `crates/bot/src/telegram/handler.rs`, add near the other newtypes (around line 28):

```rust
/// Shared timestamp of last interaction (unix seconds).
/// Updated by handler on incoming messages and by worker after sending replies.
#[derive(Clone)]
pub struct IdleTimestamp(pub Arc<std::sync::atomic::AtomicI64>);
```

- [ ] **Step 2: Update `handle_message` to touch the idle timestamp**

In `handle_message` (line 110), add after the `let text = ...` line (line 112):

```rust
// Touch idle timestamp — user sent a message
settings.idle_timestamp.store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);
```

Wait — `idle_timestamp` should be a separate DI param, not inside `settings`. Add it as a parameter to `handle_message`:

```rust
pub async fn handle_message(
    bot: BotType,
    msg: Message,
    worker_map: Arc<DashMap<SessionKey, mpsc::Sender<DebounceMsg>>>,
    agent_dir: Arc<AgentDir>,
    debug_flag: Arc<DebugFlag>,
    ssh_config: Arc<SshConfigPath>,
    auth_watcher_flag: Arc<AuthWatcherFlag>,
    auth_code_slot: Arc<AuthCodeSlot>,
    settings: Arc<AgentSettings>,
    stop_tokens: super::StopTokens,
    idle_ts: Arc<IdleTimestamp>,
) -> ResponseResult<()> {
```

And at the top of the function body:

```rust
idle_ts.0.store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 3: Pass idle timestamp into WorkerContext**

Add to `WorkerContext` in `worker.rs` (around line 83):

```rust
/// Shared idle timestamp — worker updates after each reply sent.
pub idle_timestamp: Arc<std::sync::atomic::AtomicI64>,
```

- [ ] **Step 4: Touch idle timestamp after worker sends reply**

In `spawn_worker` in `worker.rs`, after the reply is sent to Telegram (after the `send_attachments` call around line 506, and after the error reply send around line 520), add:

```rust
ctx.idle_timestamp.store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);
```

Place it right before the closing `}` of the `match reply_result` block (before line 522), so it fires for all outcomes (Ok with content, Ok(None), Err).

- [ ] **Step 5: Wire in dispatch.rs**

In `crates/bot/src/telegram/dispatch.rs`:

1. Import `IdleTimestamp` from handler.
2. Create the shared idle timestamp in `run_telegram` (around line 80):

```rust
let idle_ts = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
    chrono::Utc::now().timestamp(),
))));
```

3. Add to `dptree::deps!` (line 129):

```rust
Arc::clone(&idle_ts),
```

4. Pass to `WorkerContext` construction in `handle_message` (handler.rs, around line 168):

```rust
idle_timestamp: Arc::clone(&idle_ts.0),
```

5. Update `run_telegram` to return/expose the `idle_ts` so `lib.rs` can pass it to the delivery loop. Change the return type or add it as an output parameter. Simplest: make `run_telegram` accept `idle_ts: Arc<IdleTimestamp>` as a parameter instead of creating it internally.

- [ ] **Step 6: Update lib.rs to create and share the idle timestamp**

In `crates/bot/src/lib.rs`, create the idle timestamp before spawning cron and telegram:

```rust
use crate::telegram::handler::IdleTimestamp;
let idle_timestamp = Arc::new(IdleTimestamp(Arc::new(std::sync::atomic::AtomicI64::new(
    chrono::Utc::now().timestamp(),
))));
```

Pass `Arc::clone(&idle_timestamp)` to both `run_telegram()` and the delivery poll loop (Task 6).

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p rightclaw-bot`

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/telegram/handler.rs crates/bot/src/telegram/worker.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs
git commit -m "feat(cron): shared idle timestamp across handler/worker"
```

---

### Task 6: Delivery poll loop

**Files:**
- Create: `crates/bot/src/cron_delivery.rs`
- Modify: `crates/bot/src/lib.rs`

This is the core new component. A tokio task that:
1. Waits for 5 min idle
2. Queries undelivered cron results
3. Deduplicates (marks older same-job results as delivered)
4. Pipes the result through the main CC session
5. Sends the reply to Telegram
6. Marks as delivered, cleans up attachments

- [ ] **Step 1: Write the test for `pending_cron_result` DB query**

Create `crates/bot/src/cron_delivery.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use rusqlite::OptionalExtension as _;

use crate::telegram::handler::IdleTimestamp;

/// A pending cron result ready for delivery.
#[derive(Debug)]
pub struct PendingCronResult {
    pub id: String,
    pub job_name: String,
    pub notify_json: String,
    pub summary: String,
    pub finished_at: String,
}

/// Query the oldest undelivered cron result with a non-null notify_json.
pub fn fetch_pending(conn: &rusqlite::Connection) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs \
         WHERE status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at ASC LIMIT 1"
    )?;
    let result = stmt.query_row([], |row| {
        Ok(PendingCronResult {
            id: row.get(0)?,
            job_name: row.get(1)?,
            notify_json: row.get(2)?,
            summary: row.get(3)?,
            finished_at: row.get(4)?,
        })
    });
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Fetch a specific cron result by ID.
pub fn fetch_by_id(conn: &rusqlite::Connection, id: &str) -> Result<Option<PendingCronResult>, rusqlite::Error> {
    let result = conn.query_row(
        "SELECT id, job_name, notify_json, summary, finished_at FROM cron_runs WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(PendingCronResult {
                id: row.get(0)?,
                job_name: row.get(1)?,
                notify_json: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                finished_at: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        },
    );
    match result {
        Ok(r) => Ok(Some(r)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mark a cron run as delivered.
pub fn mark_delivered(conn: &rusqlite::Connection, run_id: &str) -> Result<(), rusqlite::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 WHERE id = ?2",
        rusqlite::params![now, run_id],
    )?;
    Ok(())
}

/// Deduplicate: for a given job, find the latest undelivered result and mark all
/// older undelivered results as delivered. Returns (latest_id, skipped_count).
/// Returns None if no undelivered results exist for this job.
pub fn deduplicate_job(
    conn: &rusqlite::Connection,
    job_name: &str,
) -> Result<Option<(String, u32)>, rusqlite::Error> {
    // Find the latest undelivered result for this job
    let latest_id: Option<String> = conn.query_row(
        "SELECT id FROM cron_runs \
         WHERE job_name = ?1 AND status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL \
         ORDER BY finished_at DESC LIMIT 1",
        rusqlite::params![job_name],
        |row| row.get(0),
    ).optional()?;

    let Some(latest_id) = latest_id else {
        return Ok(None);
    };

    // Mark all older results as delivered
    let now = chrono::Utc::now().to_rfc3339();
    let count = conn.execute(
        "UPDATE cron_runs SET delivered_at = ?1 \
         WHERE job_name = ?2 AND id != ?3 \
         AND status = 'success' AND notify_json IS NOT NULL AND delivered_at IS NULL",
        rusqlite::params![now, job_name, latest_id],
    )?;

    Ok(Some((latest_id, count as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> rusqlite::Connection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        rightclaw::memory::migrations::MIGRATIONS.to_latest(&mut conn).unwrap();
        conn
    }

    #[test]
    fn fetch_pending_empty_db() {
        let conn = setup_db();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_returns_oldest() {
        let conn = setup_db();
        // Insert two successful runs with notify_json
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"first\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"second\"}')",
            [],
        ).unwrap();

        let pending = fetch_pending(&conn).unwrap().unwrap();
        assert_eq!(pending.id, "a", "should return oldest first");
    }

    #[test]
    fn fetch_pending_skips_null_notify() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'silent')",
            [],
        ).unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn fetch_pending_skips_delivered() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json, delivered_at) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"done\"}', '2026-01-01T00:10:00Z')",
            [],
        ).unwrap();
        assert!(fetch_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn deduplicate_keeps_latest_marks_older() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum1', '{\"content\":\"old\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job1', '2026-01-01T00:05:00Z', '2026-01-01T00:06:00Z', 'success', '/log', 'sum2', '{\"content\":\"new\"}')",
            [],
        ).unwrap();

        let (latest_id, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest_id, "b", "should pick the latest by finished_at");
        assert_eq!(skipped, 1);

        // 'a' should now be delivered, 'b' should not
        let delivered: Option<String> = conn.query_row(
            "SELECT delivered_at FROM cron_runs WHERE id = 'a'", [], |r| r.get(0),
        ).unwrap();
        assert!(delivered.is_some());

        let not_delivered: Option<String> = conn.query_row(
            "SELECT delivered_at FROM cron_runs WHERE id = 'b'", [], |r| r.get(0),
        ).unwrap();
        assert!(not_delivered.is_none());
    }

    #[test]
    fn deduplicate_does_not_touch_other_jobs() {
        let conn = setup_db();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('a', 'job1', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"x\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_runs (id, job_name, started_at, finished_at, status, log_path, summary, notify_json) \
             VALUES ('b', 'job2', '2026-01-01T00:00:00Z', '2026-01-01T00:01:00Z', 'success', '/log', 'sum', '{\"content\":\"y\"}')",
            [],
        ).unwrap();

        let (latest_id, skipped) = deduplicate_job(&conn, "job1").unwrap().unwrap();
        assert_eq!(latest_id, "a");
        assert_eq!(skipped, 0, "should not touch job2");
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/bot/src/lib.rs`, add:

```rust
pub mod cron_delivery;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot cron_delivery`
Expected: All 5 DB tests pass.

- [ ] **Step 4: Commit the DB layer**

```bash
git add crates/bot/src/cron_delivery.rs crates/bot/src/lib.rs
git commit -m "feat(cron): cron_delivery module — DB query/dedup/mark functions with tests"
```

- [ ] **Step 5: Add `format_cron_yaml` function**

In `crates/bot/src/cron_delivery.rs`, add:

```rust
/// Format a pending cron result as YAML for the main CC session.
pub fn format_cron_yaml(pending: &PendingCronResult, skipped: u32) -> String {
    let total = skipped + 1;
    let mut yaml = String::new();
    yaml.push_str("cron_result:\n");
    yaml.push_str(&format!("  job: {}\n", pending.job_name));
    yaml.push_str(&format!("  runs_total: {total}\n"));
    if skipped > 0 {
        yaml.push_str(&format!("  skipped_runs: {skipped}\n"));
    }

    // Parse notify_json to inline content and attachments
    if let Ok(notify) = serde_json::from_str::<serde_json::Value>(&pending.notify_json) {
        yaml.push_str("  result:\n");
        yaml.push_str("    notify:\n");
        if let Some(content) = notify.get("content").and_then(|v| v.as_str()) {
            yaml.push_str(&format!("      content: \"{}\"\n", content.replace('"', "\\\"")));
        }
        if let Some(atts) = notify.get("attachments").and_then(|v| v.as_array()) {
            if !atts.is_empty() {
                yaml.push_str("      attachments:\n");
                for att in atts {
                    let att_type = att.get("type").and_then(|v| v.as_str()).unwrap_or("document");
                    let path = att.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    yaml.push_str(&format!("        - type: {att_type}\n"));
                    yaml.push_str(&format!("          path: {path}\n"));
                    if let Some(caption) = att.get("caption").and_then(|v| v.as_str()) {
                        yaml.push_str(&format!("          caption: \"{}\"\n", caption.replace('"', "\\\"")));
                    }
                }
            }
        }
        yaml.push_str(&format!("    summary: \"{}\"\n", pending.summary.replace('"', "\\\"")));
    }

    yaml
}
```

- [ ] **Step 6: Write test for `format_cron_yaml`**

```rust
#[test]
fn format_cron_yaml_basic() {
    let pending = PendingCronResult {
        id: "abc".into(),
        job_name: "health-check".into(),
        notify_json: r#"{"content":"BTC up 2%"}"#.into(),
        summary: "Checked 5 pairs".into(),
        finished_at: "2026-01-01T00:01:00Z".into(),
    };
    let yaml = format_cron_yaml(&pending, 2);
    assert!(yaml.contains("job: health-check"));
    assert!(yaml.contains("runs_total: 3"));
    assert!(yaml.contains("skipped_runs: 2"));
    assert!(yaml.contains("BTC up 2%"));
    assert!(yaml.contains("Checked 5 pairs"));
}

#[test]
fn format_cron_yaml_no_skipped() {
    let pending = PendingCronResult {
        id: "abc".into(),
        job_name: "job1".into(),
        notify_json: r#"{"content":"hello"}"#.into(),
        summary: "done".into(),
        finished_at: "2026-01-01T00:01:00Z".into(),
    };
    let yaml = format_cron_yaml(&pending, 0);
    assert!(yaml.contains("runs_total: 1"));
    assert!(!yaml.contains("skipped_runs"));
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p rightclaw-bot format_cron_yaml`
Expected: Pass.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron): format_cron_yaml for delivery through main session"
```

- [ ] **Step 9: Add the `run_delivery_loop` function**

In `crates/bot/src/cron_delivery.rs`, add:

```rust
const IDLE_THRESHOLD_SECS: i64 = 300; // 5 minutes
const POLL_INTERVAL_SECS: u64 = 30;   // Check every 30s

/// Main delivery loop. Runs as a tokio task.
///
/// Waits for idle (5 min no interaction), then delivers pending cron results
/// one at a time through the main CC session.
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    model: Option<String>,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tracing::info!(agent = %agent_name, "cron delivery loop started");

    loop {
        // Poll every 30s or until shutdown
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)) => {}
            _ = shutdown.cancelled() => {
                tracing::info!("cron delivery loop shutting down");
                return;
            }
        }

        // Check idle
        let last = idle_ts.0.load(std::sync::atomic::Ordering::Relaxed);
        let now = chrono::Utc::now().timestamp();
        if now - last < IDLE_THRESHOLD_SECS {
            continue; // Not idle yet
        }

        // Open DB connection
        let conn = match rightclaw::memory::open_connection(&agent_dir) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("cron delivery: DB open failed: {e:#}");
                continue;
            }
        };

        // Fetch oldest pending result to get the job name
        let pending = match fetch_pending(&conn) {
            Ok(Some(p)) => p,
            Ok(None) => continue, // Nothing to deliver
            Err(e) => {
                tracing::error!("cron delivery: fetch_pending failed: {e:#}");
                continue;
            }
        };

        // Deduplicate: find latest for this job, mark older ones as delivered
        let (latest_id, skipped) = match deduplicate_job(&conn, &pending.job_name) {
            Ok(Some((id, s))) => (id, s),
            Ok(None) => continue, // All deduped away (shouldn't happen)
            Err(e) => {
                tracing::error!("cron delivery: deduplicate failed: {e:#}");
                continue;
            }
        };

        // Fetch the actual latest result by ID
        let to_deliver = match fetch_by_id(&conn, &latest_id) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("cron delivery: fetch_by_id failed: {e:#}");
                continue;
            }
        };

        let yaml = format_cron_yaml(&to_deliver, skipped);
        tracing::info!(
            job = %to_deliver.job_name,
            run_id = %to_deliver.id,
            skipped,
            "delivering cron result through main session"
        );

        // Deliver through main CC session
        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            model.as_deref(),
            &bot,
            &notify_chat_ids,
            ssh_config_path.as_deref(),
        ).await {
            Ok(()) => {
                // Mark as delivered
                if let Err(e) = mark_delivered(&conn, &to_deliver.id) {
                    tracing::error!(run_id = %to_deliver.id, "mark_delivered failed: {e:#}");
                }

                // Cleanup outbox for this run
                let outbox_dir = agent_dir.join("outbox").join("cron").join(&to_deliver.id);
                if outbox_dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&outbox_dir) {
                        tracing::warn!(run_id = %to_deliver.id, "outbox cleanup failed: {e:#}");
                    }
                }

                // Touch idle — delivery counts as interaction
                idle_ts.0.store(chrono::Utc::now().timestamp(), std::sync::atomic::Ordering::Relaxed);
            }
            Err(e) => {
                tracing::error!(
                    job = %to_deliver.job_name,
                    run_id = %to_deliver.id,
                    "cron delivery failed: {e:#}"
                );
                // Don't mark as delivered — will retry next poll
            }
        }
    }
}
```

- [ ] **Step 10: Add `deliver_through_session` function**

This function invokes `claude -p --resume <session_id>` for the first `notify_chat_id`, piping the YAML as input. Then sends the reply to Telegram.

```rust
/// Invoke the main CC session with cron result YAML and send the reply to Telegram.
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    model: Option<&str>,
    bot: &crate::telegram::BotType,
    notify_chat_ids: &[i64],
    ssh_config_path: Option<&Path>,
) -> Result<(), String> {
    use std::process::Stdio;

    if notify_chat_ids.is_empty() {
        return Err("no notify_chat_ids configured".into());
    }

    let chat_id = notify_chat_ids[0];
    let eff_thread_id: i64 = 0; // Main thread

    // Look up active session for this chat
    let conn = rightclaw::memory::open_connection(agent_dir)
        .map_err(|e| format!("DB open: {e:#}"))?;

    let session_id = crate::telegram::session::get_active_session(&conn, chat_id, eff_thread_id)
        .map_err(|e| format!("session lookup: {e:#}"))?
        .map(|s| s.root_session_id);

    // Build CC command
    let cc_bin = which::which("claude")
        .or_else(|_| which::which("claude-bun"))
        .map_err(|_| "claude binary not found in PATH".to_string())?;

    let mut cmd = tokio::process::Command::new(&cc_bin);
    cmd.arg("-p");
    cmd.arg("--dangerously-skip-permissions");
    cmd.arg("--agent").arg(agent_name);
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    // Low budget — delivery is just formatting, not real work
    cmd.arg("--max-budget-usd").arg("0.05");
    cmd.arg("--max-turns").arg("3");
    cmd.arg("--output-format").arg("json");

    // Resume existing session if available
    if let Some(ref sid) = session_id {
        cmd.arg("--resume").arg(sid);
    }

    // Reply schema — agent should respond with standard reply format
    let reply_schema_path = agent_dir.join(".claude").join("reply-schema.json");
    if let Ok(schema) = std::fs::read_to_string(&reply_schema_path) {
        cmd.arg("--json-schema").arg(schema);
    }

    cmd.env("HOME", agent_dir);
    cmd.env("USE_BUILTIN_RIPGREP", "0");
    cmd.current_dir(agent_dir);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e:#}"))?;

    // Write YAML input
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(yaml_input.as_bytes()).await
            .map_err(|e| format!("stdin write: {e:#}"))?;
    }

    let output = child.wait_with_output().await
        .map_err(|e| format!("wait_with_output: {e:#}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("CC exited with {}: {stderr}", output.status));
    }

    // Parse reply using existing worker parser
    let raw = String::from_utf8_lossy(&output.stdout);
    let (reply, _) = crate::telegram::worker::parse_reply_output(&raw)
        .map_err(|e| format!("reply parse: {e}"))?;

    // Send text to Telegram
    if let Some(ref content) = reply.content {
        use teloxide::prelude::Requester as _;
        for &cid in notify_chat_ids {
            if let Err(e) = bot.send_message(teloxide::types::ChatId(cid), content).await {
                tracing::error!(chat_id = cid, "cron delivery: Telegram send failed: {e:#}");
            }
        }
    }

    // Send attachments
    if let Some(ref atts) = reply.attachments {
        if !atts.is_empty() {
            for &cid in notify_chat_ids {
                if let Err(e) = crate::telegram::attachments::send_attachments(
                    atts,
                    bot,
                    teloxide::types::ChatId(cid),
                    0,
                    agent_dir,
                    ssh_config_path,
                    agent_name,
                ).await {
                    tracing::error!(chat_id = cid, "cron delivery: attachment send failed: {e:#}");
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 11: Verify compilation**

Run: `cargo check -p rightclaw-bot`

- [ ] **Step 12: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "feat(cron): delivery poll loop — idle detection, CC session delivery, cleanup"
```

---

### Task 7: Wire delivery loop into bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Spawn the delivery loop alongside cron task**

In `crates/bot/src/lib.rs`, after the cron spawn block (around line 196), add:

```rust
// Cron delivery loop: delivers pending cron results through main CC session when idle
let delivery_agent_dir = agent_dir.clone();
let delivery_agent_name = args.agent.clone();
let delivery_model = config.model.clone();
let delivery_bot = telegram::bot::build_bot(token.clone());
let delivery_chat_ids = config.allowed_chat_ids.clone();
let delivery_idle_ts = Arc::clone(&idle_timestamp);
let delivery_ssh_config = ssh_config_path.clone();
let delivery_shutdown = shutdown.clone();
let delivery_handle = tokio::spawn(async move {
    cron_delivery::run_delivery_loop(
        delivery_agent_dir,
        delivery_agent_name,
        delivery_model,
        delivery_bot,
        delivery_chat_ids,
        delivery_idle_ts,
        delivery_ssh_config,
        delivery_shutdown,
    ).await;
});
```

- [ ] **Step 2: Await the delivery handle in shutdown**

Find the existing shutdown wait block and add `delivery_handle` to it. If there's a `tokio::select!` or `join!` for `cron_handle`, add `delivery_handle` alongside it.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p rightclaw-bot`

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(cron): wire delivery loop into bot startup"
```

---

### Task 8: Write cron-schema.json to disk during codegen

**Files:**
- Modify: `crates/bot/src/lib.rs` or the codegen site where schemas are written

The cron schema needs to be available as a file for `execute_job` to pass to `--json-schema`.

- [ ] **Step 1: Find where reply-schema.json is written**

Search for where `REPLY_SCHEMA_JSON` is written to disk. This is likely in the bot's codegen/sync module.

- [ ] **Step 2: Add cron-schema.json alongside reply-schema.json**

At the same location, add:

```rust
std::fs::write(
    agent_dir.join(".claude").join("cron-schema.json"),
    rightclaw::codegen::CRON_SCHEMA_JSON,
)?;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p rightclaw-bot`

- [ ] **Step 4: Commit**

```bash
git add <files>
git commit -m "feat(cron): write cron-schema.json during codegen"
```

---

### Task 9: Build full workspace and run all tests

**Files:** None (verification only)

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace`
Expected: Clean build, no errors.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings in new code.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass.

- [ ] **Step 4: Commit any clippy fixes**

```bash
git add -A
git commit -m "fix: address clippy warnings from cron feedback redesign"
```

---

### Task 10: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update the cron section**

Add `cron_delivery.rs` to the module map under `rightclaw-bot`:

```
├── cron_delivery.rs    # Delivery poll loop: idle detection, CC session delivery, cleanup
```

Update the Data Flow section to reflect the new delivery path.

- [ ] **Step 2: Update DB schema section**

Add the new columns to the `cron_runs` schema:

```
cron_runs       (id, job_name, started_at, finished_at, exit_code, status, log_path, summary, notify_json, delivered_at)
```

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update ARCHITECTURE.md for cron feedback redesign"
```
