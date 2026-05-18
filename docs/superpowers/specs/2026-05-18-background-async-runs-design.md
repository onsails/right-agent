# Background Handoff and Unified Async Runs

**Date:** 2026-05-18
**Status:** Design (approved; pending implementation plan)

## Problem

The current background continuation path is implemented as a cron job. When a
foreground turn is sent to background, the worker cancels the foreground
`claude -p`, inserts a `BackgroundContinuation` cron spec, edits the Telegram
banner, and returns to normal foreground processing. The actual
`--fork-session` happens later in the cron executor.

That creates a race:

1. User taps Background for message A.
2. Worker enqueues `@bg:<main_session_id>` and releases the thread.
3. User immediately sends message B.
4. Foreground resumes and mutates the same main session with B.
5. Cron later forks from the now-mutated main session.
6. The background continuation answers B instead of A.

There is a second correctness issue in delivery: the current cron delivery path
can mark a run delivered even when no Telegram message was actually sent.

## Goals

- Make Background a background execution concern, not a cron scheduling
  concern.
- Ensure no foreground turn can mutate the main session between background
  cancellation and confirmed background fork handoff.
- Use one neutral execution/result table for cron runs and background runs.
- Delete `cron_runs` in a migration; no dual reads, mirror writes, or fallback
  runtime paths.
- Keep the public cron MCP surface intact by reading cron history from the new
  neutral run table.
- Generalize delivery so cron and background results use one delivery system.
- Mark delivery as successful only after a confirmed Telegram send/edit.

## Non-goals

- Keeping the original foreground process alive as the background worker. That
  would require detaching a process that was launched with foreground
  capabilities and moving foreground ownership to a new session branch.
- Blocking the thread until a background run completes. The gate is only for the
  handoff window.
- Adding new user-facing MCP tools for background history.
- Rewriting the entire cron scheduler. Cron remains the scheduler for cron
  specs; only execution history and delivery are unified.

## Decisions

| Area | Decision |
| --- | --- |
| Background ownership | Worker starts background execution immediately after cancel, not through the cron reconcile tick. |
| Handoff gate | Gate `(chat_id, thread_id)` until the background fork handoff is confirmed. |
| Fork confirmation | Do not release on OS spawn alone. Release after a stronger observable signal, expected to be the first `system/init` stream event for the background run session. Implementation must verify this signal really happens after `--fork-session` has selected the forked session. |
| Session ordering | Hold the existing `SessionLocks` entry for `source_session_id` during the handoff window so foreground, delivery, and background fork startup cannot race the same main JSONL. |
| Storage | Introduce `async_runs` as the single execution/result/delivery history table. |
| Cron run history | `cron_list_runs` and `cron_get_run` keep the same MCP API but query `async_runs WHERE kind = 'cron'`. |
| Legacy table | Migration copies `cron_runs` into `async_runs`, then drops `cron_runs`. |
| Delivery | Delivery loop reads only `async_runs`. |
| Silent cron | Represent with `delivery_required = false` and `delivery_status = 'none'`. |

## Architecture

### Background handoff executor

The worker owns background handoff. It is responsible for:

- ensuring a per-thread handoff gate is set before the foreground child is
  cancelled;
- cancelling the current foreground child through the existing stop token path;
- creating an `async_runs` row for the background run;
- launching `claude -p --resume <main_session_id> --fork-session --session-id <bg_run_id>`;
- holding the main session lock until fork handoff is confirmed;
- releasing the gate after confirmed handoff or after a handled handoff failure.

The executor does not wait for the background run to finish before releasing the
thread.

### Async run storage

`async_runs` records execution state, structured output, and delivery state for
all async execution kinds. Cron and background share this table.

`cron_specs` continues to own schedule definitions only. It no longer owns
background continuations, and delivery no longer reads cron-specific run rows.

### Shared delivery

The delivery loop relays completed `async_runs` to Telegram/main sessions. It
does not care whether the producer was cron or background except when choosing
the delivery instruction text.

## Data Model

Create a new `async_runs` table:

```text
id                    TEXT PRIMARY KEY
kind                  TEXT NOT NULL        -- cron | background
producer_ref           TEXT                 -- cron job_name for cron
source_session_id      TEXT                 -- background fork source
run_session_id         TEXT NOT NULL         -- claude --session-id
target_chat_id         INTEGER NOT NULL
target_thread_id       INTEGER

status                TEXT NOT NULL         -- queued | running | success | failed
handoff_state          TEXT                 -- null for cron; queued | spawned for background
started_at             TEXT
finished_at            TEXT
exit_code              INTEGER
log_path               TEXT

summary                TEXT
notify_json            TEXT
no_notify_reason       TEXT
error_json             TEXT

delivery_required      INTEGER NOT NULL     -- bool
delivery_status        TEXT NOT NULL        -- none | pending | retryable | delivered | failed | superseded
delivery_attempts      INTEGER NOT NULL DEFAULT 0
delivered_at           TEXT
last_delivery_error    TEXT
created_at             TEXT NOT NULL
updated_at             TEXT NOT NULL
```

For cron:

- `kind = 'cron'`
- `producer_ref = job_name`
- `source_session_id = NULL`
- `run_session_id = id`
- `handoff_state = NULL`
- `delivery_required = notify_json IS NOT NULL` after output is parsed
- silent output uses `delivery_required = false`, `delivery_status = 'none'`

For background:

- `kind = 'background'`
- `producer_ref` is optional debug label
- `source_session_id = main_session_id`
- `run_session_id = bg_run_id`
- `handoff_state = 'queued'` before confirmed handoff
- `delivery_required = true` once handoff is confirmed
- `delivery_status = 'pending'`

Indexes:

- `(kind, producer_ref, started_at DESC)` for `cron_list_runs`
- `(delivery_required, delivery_status, status, finished_at)` for delivery
- `(target_chat_id, target_thread_id, status)` for diagnostics and future UI
- `(run_session_id)` for session lookup/debugging

## Migration

The migration is one-way and removes the old active table:

```text
BEGIN
  CREATE TABLE async_runs (...)
  INSERT INTO async_runs (...)
    SELECT ... FROM cron_runs
  DROP TABLE cron_runs
  CREATE INDEX ...
COMMIT
```

Mapping from `cron_runs`:

- ordinary rows become `kind = 'cron'`, `producer_ref = job_name`;
- old background rows are best-effort detected by matching an existing
  `cron_specs.schedule LIKE '@bg:%'` row or by the generated `bg-` job name
  prefix, and become `kind = 'background'`;
- all fields used by cron MCP and delivery are preserved: `summary`,
  `notify_json`, `no_notify_reason`, `delivery_status`, `delivered_at`,
  `target_chat_id`, `target_thread_id`, `log_path`, `exit_code`, status, and
  timestamps;
- old `delivery_status = 'silent'` maps to `delivery_status = 'none'`.

Pending legacy `@bg:<uuid>` cron specs are not kept as schedulable crons. They
are migrated into failed background `async_runs` rows with
`delivery_required = true` and a generated `id`/`run_session_id`, then the cron
spec is removed. This avoids silently dropping user-visible work while still
eliminating the old cron-backed background runtime path.

After migration:

- runtime code does not read or write `cron_runs`;
- tests should assert the table no longer exists;
- `cron_specs` contains only real cron schedule definitions.

## Background Handoff Flow

Target sequence:

```text
gate thread
cancel foreground child
create async_runs row
start background continuation immediately
wait until fork handoff is confirmed
ungate thread
```

Detailed flow:

1. Background button sets the current turn's background request, sets the
   per-thread handoff gate for `(chat_id, thread_id)`, and cancels the stop
   token.
2. The worker exits the foreground invocation as `Backgrounded`.
3. The worker takes ownership of the existing handoff gate. Incoming messages
   may still be accepted into the normal queue, but no new foreground
   `claude -p` may start for that key while the gate is held.
4. The worker retains the user message using the existing background retain
   behavior.
5. The worker inserts `async_runs(kind='background', status='queued',
   handoff_state='queued')`.
6. The worker acquires `SessionLocks[source_session_id]`.
7. The worker starts the background child:

```text
claude -p
  --resume <source_session_id>
  --fork-session
  --session-id <run_session_id>
  --output-format stream-json
  --json-schema BG_CONTINUATION_SCHEMA_JSON
```

8. The stdout/stderr reader task takes ownership of the child.
9. The worker waits for the handoff confirmation signal. The expected signal is
   the first valid `system/init` event for `run_session_id`; implementation must
   verify this is emitted only after the CLI has selected the forked session.
10. On confirmation, update the row to `status='running'`,
    `handoff_state='spawned'`, edit the Telegram thinking banner, release the
    session lock, and release the thread gate.
11. The background task later parses completion output and updates
    `async_runs` with `success`/`failed`, `notify_json`/`error_json`,
    `finished_at`, `exit_code`, and `log_path`.

If the handoff confirmation does not arrive within a short timeout, kill the
background child, mark the run failed, send/edit an immediate handoff error, and
release the gate. Do not release the gate while an unconfirmed child might still
fork from the main session.

## Shared Delivery

Delivery queries only `async_runs`:

```text
delivery_required = true
delivery_status IN ('pending', 'retryable')
status IN ('success', 'failed')
```

For each row, delivery:

1. resolves the target chat/thread;
2. resolves the active main session for that target;
3. acquires `SessionLocks[main_session_id]`;
4. formats a delivery prompt based on `(kind, status)`;
5. invokes the cheap relay model against the main session;
6. sends the resulting content and attachments to Telegram;
7. marks delivered only after a real Telegram send/edit succeeds.

Instruction variants:

- `kind='cron', status='success'`: existing cron success relay behavior.
- `kind='cron', status='failed'`: existing cron failure relay behavior.
- `kind='background', status='success'`: relay the background answer as the
  answer to the user request that was moved to background.
- `kind='background', status='failed'`: report that the background continuation
  failed, preserving diagnostic facts.

Delivery must not return success when:

- `notify_json` is null for a delivery-required run;
- background content is empty;
- Telegram send fails and fallback send also fails;
- all attachments fail and there is no successfully sent text fallback.

Failures increment `delivery_attempts`, set `delivery_status='retryable'` when
future retries make sense, and store `last_delivery_error`. Permanent failures
set `delivery_status='failed'`.

## Failure and Recovery

### Handoff spawn or confirmation fails

- Mark `async_runs.status='failed'`.
- Set `delivery_required=false` when the live worker can edit/send an immediate
  error to the user.
- Release the session lock and thread gate.

### Child starts, then fails

- Mark `status='failed'`.
- Store structured error/reflection output in `error_json` or `notify_json`.
- Keep `delivery_required=true`.
- Shared delivery reports the failure.

### Bot crashes before handoff confirmation

On startup, rows with:

```text
kind='background'
status='queued'
handoff_state='queued'
```

are treated as interrupted handoffs, not automatically replayed. Mark them
failed with `delivery_required=true`, so the user receives a failure report
instead of silent loss. Automatic retry is unsafe because the original action may
have had side effects.

### Bot crashes while async run is running

Rows stuck in `status='running'` need stale-run recovery. The first version
should not retry automatically. If heartbeat/process ownership is stale past the
configured threshold, mark failed and let shared delivery report the failure.

### Delivery fails

Delivery failures never mark a run delivered. They update attempts and error
state, then retry according to the existing delivery loop cadence/backoff.

## Cron MCP Compatibility

Public tool names and user-facing semantics stay:

- `mcp__right__cron_list`
- `mcp__right__cron_list_runs`
- `mcp__right__cron_get_run` / existing run detail API

Implementation changes:

- list current jobs from `cron_specs`;
- compute last-run status from latest `async_runs` row where
  `kind='cron' AND producer_ref=cron_specs.job_name`;
- list run history from `async_runs WHERE kind='cron'`;
- fetch run detail from `async_runs WHERE kind='cron' AND id=?`;
- never expose background rows through cron history.

## Testing Plan

### Migration tests

- `cron_runs` rows migrate into `async_runs(kind='cron')`.
- Old background rows migrate into `async_runs(kind='background')` when
  detectable.
- All fields needed by cron MCP and delivery are preserved.
- `cron_runs` is dropped.
- Indexes exist.
- Legacy `@bg:<uuid>` cron specs are removed or converted to failed
  background rows.

### Cron MCP tests

- `cron_list_runs` reads only `async_runs WHERE kind='cron'`.
- Background rows never appear in cron history.
- Run detail returns cron rows from `async_runs`.
- `/cron` last-run status is computed from `async_runs`.

### Background handoff tests

- Background creates an `async_runs(kind='background')` row.
- Next foreground turn cannot start while the handoff gate is held.
- Gate is released after confirmed handoff, not after run completion.
- Gate is not released on OS spawn alone.
- Spawn/init timeout releases the gate only after the child is killed and the
  run is marked failed.

### Delivery tests

- Delivery reads `async_runs`, not `cron_runs`.
- Silent cron runs are ignored with `delivery_status='none'`.
- Delivery-required runs with null/empty notify are not marked delivered.
- Telegram send failure does not mark delivered.
- Successful send marks delivered and records `delivered_at`.

### Regression scenario

Use fake executor/fake delivery, not real Claude:

```text
message A starts foreground
user taps Background
message B arrives immediately
background continuation answers A
foreground answers B
```

The test should fail against the current cron-backed design because B can enter
the main session before the background fork. It should pass when the handoff
gate and confirmed immediate background fork are in place.

## Documentation and Prompt Updates

Update the architecture docs touched by this subsystem:

- `docs/architecture/sessions.md`
- `docs/architecture/lifecycle.md`
- `ARCHITECTURE.md` if load-bearing rules change

Update `PROMPT_SYSTEM.md` if implementation changes agent-facing background,
cron, delivery, schema, or MCP instruction text.

## Verification Cadence

During implementation:

- start with targeted failing regression tests;
- run targeted crate/module tests after coherent implementation slices;
- avoid full workspace tests after every small edit;
- before declaring completion, run `devenv shell -- cargo test --workspace`;
- because this is Rust behavior work, run the Rust review flow required by the
  repository instructions if the required skill/subagent is available in that
  session.
