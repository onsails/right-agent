---
name: rightcron
description: >-
  Manages cron jobs for this RightClaw agent via MCP tools. Creates, updates,
  and deletes cron specs stored in the agent database. The Rust runtime handles
  scheduling and execution automatically. Use when the user mentions cron
  jobs, scheduled tasks, RightCron, or recurring tasks.
version: 2.0.0
---

# /rightcron -- Cron Job Manager

## When to Activate

Activate this skill when:
- The user mentions "cron", "cron jobs", "scheduled tasks", or "RightCron"
- The user asks to schedule, create, remove, or change a recurring task
- The user asks about cron run history or why a job failed

## How It Works

Cron specs are stored in the agent database. The Rust runtime polls specs every 60 seconds and schedules jobs automatically. Use MCP tools to manage specs — no file creation needed.

## Creating a Cron Job

Use the `cron_create` MCP tool:

```
cron_create(
  job_name: "health-check",
  schedule: "17 9 * * 1-5",
  prompt: "Check system health and report status",
  max_budget_usd: 0.50
)
```

Confirm to the user: "Job created. The runtime picks up new specs within ~60 seconds."

## Editing a Cron Job

Use the `cron_update` MCP tool (full replacement — all fields required):

```
cron_update(
  job_name: "health-check",
  schedule: "43 */4 * * *",
  prompt: "Check system health, alert on degradation",
  max_budget_usd: 0.75
)
```

Confirm: "Job updated. Changes take effect within ~60 seconds."

## Removing a Cron Job

Use the `cron_delete` MCP tool:

```
cron_delete(job_name: "health-check")
```

Confirm: "Job removed. The runtime drops it within ~60 seconds."

## Triggering a Cron Job Manually

Use the `cron_trigger` MCP tool to run a job immediately:

```
cron_trigger(job_name: "health-check")
```

The job is queued and executes on the next engine tick (≤30s). Lock check still applies — if the job is currently running, the trigger is skipped. The result is delivered through the normal delivery loop.

Confirm: "Job triggered. Execution starts within ~30 seconds."

## Listing Current Cron Jobs

Use the `cron_list` MCP tool to see all configured jobs:

```
cron_list()
```

Returns: job_name, schedule, prompt, lock_ttl, max_budget_usd for each job.

## Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `job_name` | string | Yes | - | Lowercase alphanumeric and hyphens (e.g. `health-check`). |
| `schedule` | string | Yes | - | Standard 5-field cron expression (minute hour day-of-month month day-of-week). Evaluated in **UTC**. |
| `prompt` | string | Yes | - | The task prompt that Claude executes when the cron fires. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g. `10m`, `1h`). |
| `max_budget_usd` | number | No | `1.0` | Maximum dollar spend per invocation. Claude stops gracefully when budget is reached. |

### Schedule Guidelines

When the user doesn't specify exact minutes, **avoid :00 and :30** — these are peak times when many automated jobs fire simultaneously, causing API rate limit spikes. Use odd minutes like `:17`, `:43`, `:07`, `:53` to spread load.

The tool returns a warning when it detects `:00` or `:30` in the minute field.

## Checking Run History

Use the `rightclaw` MCP server tools to check cron job execution history.

### cron_list_runs

Returns recent runs sorted by `started_at` descending.

Parameters:
- `job_name` (optional string) — filter by job name; omit to return all jobs
- `limit` (optional integer) — max runs to return; default 20

Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`

### cron_show_run

Returns full metadata for a single run.

Parameters:
- `run_id` (string, UUID) — run to retrieve

### Reading logs

The `log_path` field in each run record points to the log file. Read it directly:

```
cat <log_path>
```

### Debugging example

```
User: "Why did morning-briefing fail?"

1. cron_list_runs(job_name="morning-briefing", limit=5)
   -> Find the failed run (status="failed")
2. cron_show_run(run_id="<run_id from step 1>")
   -> Get full metadata including log_path
3. cat <log_path>
   -> Read the subprocess output to diagnose the failure
```

## Constraints

1. **UTC schedules**: Cron expressions are evaluated in UTC by the Rust runtime.
2. **60-second polling**: The runtime re-reads specs every 60 seconds. After creating, editing, or deleting a spec, changes take effect within ~1 minute.
3. **Manual triggers**: `cron_trigger` queues the job; it runs on the next 60-second engine tick. If the job is locked (still running from a previous invocation), the trigger is skipped.
