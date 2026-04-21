---
name: rightcron
description: >-
  Manages cron jobs for this RightClaw agent via MCP tools. Creates, updates,
  and deletes cron specs stored in the agent database. The Rust runtime handles
  scheduling and execution automatically. Use when the user mentions cron
  jobs, scheduled tasks, reminders, one-shot tasks, or recurring tasks.
version: 3.2.0
---

# /rightcron -- Cron Job Manager

## When to Activate

Activate this skill when:
- The user mentions "cron", "cron jobs", "scheduled tasks", "reminders", or "RightCron"
- The user asks to schedule, create, remove, or change a recurring or one-shot task
- The user asks to run something at a specific time or after a delay
- The user asks about cron run history or why a job failed

## How It Works

Cron specs are stored in the agent database. The Rust runtime polls specs every 60 seconds and schedules jobs automatically. Use MCP tools to manage specs — no file creation needed.

## Creating a Cron Job

`max_budget_usd` caps the dollar spend per invocation — Claude stops gracefully when the budget is reached. The default is `$2.0`, which covers most jobs including multi-step research. Only override it when the user explicitly asks for a different cap or the task is unusually expensive.

Three job types are supported:

### Recurring (default)

```
mcp__right__cron_create(
  job_name: "health-check",
  schedule: "17 9 * * 1-5",
  prompt: "Check system health and report status"
)
```

### One-shot cron (fire once at next match, then auto-delete)

```
mcp__right__cron_create(
  job_name: "deploy-check",
  schedule: "30 15 * * *",
  recurring: false,
  prompt: "Verify deployment completed successfully"
)
```

### Run at specific time (fire once, then auto-delete)

```
mcp__right__cron_create(
  job_name: "remind-deploy",
  run_at: "2026-04-15T15:30:00Z",
  prompt: "Remind the user to review PR #42"
)
```

Confirm to the user: "Job created. The runtime picks up new specs within ~60 seconds."

## Editing a Cron Job

Use the `mcp__right__cron_update` MCP tool. Only pass the fields you want to change — unspecified fields keep their current values.

```
# Change only the prompt
mcp__right__cron_update(job_name: "health-check", prompt: "New check prompt")

# Change only the schedule
mcp__right__cron_update(job_name: "health-check", schedule: "43 */4 * * *")

# Switch from recurring to one-shot run_at
mcp__right__cron_update(job_name: "health-check", run_at: "2026-04-16T10:00:00Z")
```

Setting `schedule` clears `run_at`. Setting `run_at` clears `schedule` and forces `recurring=false`.

Confirm: "Job updated. Changes take effect within ~60 seconds."

## Removing a Cron Job

Use the `mcp__right__cron_delete` MCP tool:

```
mcp__right__cron_delete(job_name: "health-check")
```

Confirm: "Job removed. The runtime drops it within ~60 seconds."

## Triggering a Cron Job Manually

Use the `mcp__right__cron_trigger` MCP tool to run a job immediately:

```
mcp__right__cron_trigger(job_name: "health-check")
```

The job is queued and executes on the next engine tick (≤60s). Lock check still applies — if the job is currently running, the trigger is skipped. The result is delivered through the normal delivery loop.

Confirm: "Job triggered. Execution starts within ~60 seconds."

## Listing Current Cron Jobs

Use the `mcp__right__cron_list` MCP tool to see all configured jobs:

```
mcp__right__cron_list()
```

Returns: job_name, schedule, prompt, lock_ttl, max_budget_usd, recurring, run_at for each job.

## Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_name` | string | Yes | - | Lowercase alphanumeric and hyphens (e.g. `health-check`). |
| `schedule` | string | Conditional | - | 5-field cron expression (minute hour day-of-month month day-of-week) in **UTC**. Required if `run_at` not set. Mutually exclusive with `run_at`. |
| `run_at` | string | Conditional | - | ISO8601 UTC datetime (e.g. `2026-04-15T15:30:00Z`). Fire once at this time, then auto-delete. Required if `schedule` not set. Mutually exclusive with `schedule`. |
| `recurring` | boolean | No | `true` | If `false` with `schedule`, fires once at next match then auto-deletes. Ignored if `run_at` is set. |
| `prompt` | string | Yes | - | The task prompt that Claude executes when the cron fires. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g. `10m`, `1h`). |
| `max_budget_usd` | number | No | `2.0` | Maximum dollar spend per invocation. Claude stops gracefully when budget is reached. |

## One-Shot Job Behavior

One-shot jobs (both `run_at` and `recurring: false`) auto-delete from `cron_specs` after execution.
- `cron_list` shows pending one-shot specs until they fire
- `cron_list_runs` shows execution history (runs are preserved even after spec deletion)
- If the bot was offline when `run_at` passed, the job fires on the next startup

### Schedule Guidelines

When the user doesn't specify exact minutes, **avoid :00 and :30** — these are peak times when many automated jobs fire simultaneously, causing API rate limit spikes. Use odd minutes like `:17`, `:43`, `:07`, `:53` to spread load.

The tool returns a warning when it detects `:00` or `:30` in the minute field.

## Checking Run History

Use the `rightclaw` MCP server tools to check cron job execution history.

### mcp__right__cron_list_runs

Returns recent runs sorted by `started_at` descending.

Parameters:
- `job_name` (optional string) — filter by job name; omit to return all jobs
- `limit` (optional integer) — max runs to return; default 20

Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`, `summary`, `notify`, `delivered_at`, `delivery_status`, `no_notify_reason`

**Delivery diagnostics:**
- `delivery_status`: lifecycle state — `silent` (CC decided nothing to report), `pending` (awaiting delivery), `delivered` (sent to Telegram), `superseded` (newer run replaced this one), `failed` (delivery gave up after retries)
- `no_notify_reason`: CC's explanation when `notify` is null (e.g. "No changes since last run")
- `delivered_at`: timestamp when the result was delivered, or null

### mcp__right__cron_show_run

Returns full metadata for a single run.

Parameters:
- `run_id` (string, UUID) — run to retrieve

### Reading logs

The `log_path` field in each run record points to the NDJSON log file inside the agent's working directory. Read the tail to see recent activity:

```
Read(file_path: "<log_path>")
```

For large logs, prefer reading just the tail — use a high `offset` value to skip to the end.

### Debugging example

```
User: "Why did morning-briefing fail?"

1. mcp__right__cron_list_runs(job_name="morning-briefing", limit=5)
   -> Find the failed run (status="failed")
2. mcp__right__cron_show_run(run_id="<run_id from step 1>")
   -> Get full metadata including log_path
3. Read(file_path: "<log_path>")
   -> Read the tail of the log to diagnose the failure
```

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

## Watching a Running Job

To see what a cron job is currently doing:

1. Find the running job:
```
mcp__right__cron_list_runs(job_name="health-check", limit=1)
```
Check the `status` field — `"running"` means the job is active.

2. Read the tail of the log file to see current activity:
```
Read(file_path: "<log_path from step 1>")
```
The log is NDJSON (one JSON event per line) — look for `"type": "assistant"` events to see what the job is doing.

3. To follow progress, read the tail again after some time has passed.

## Constraints

1. **UTC schedules**: Cron expressions are evaluated in UTC by the Rust runtime.
2. **60-second polling**: The runtime re-reads specs every 60 seconds. After creating, editing, or deleting a spec, changes take effect within ~1 minute.
3. **Manual triggers**: `mcp__right__cron_trigger` queues the job; it runs on the next 60-second engine tick. If the job is locked (still running from a previous invocation), the trigger is skipped.
