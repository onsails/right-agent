---
name: rightcron
description: >-
  Manages cron job spec files for this RightClaw agent. Creates, edits, and
  deletes YAML spec files in the crons/ directory. The Rust runtime handles
  scheduling and execution automatically. Use when the user mentions cron
  jobs, scheduled tasks, RightCron, or recurring tasks.
version: 1.0.0
---

# /rightcron -- Cron Job File Manager

## When to Activate

Activate this skill when:
- The user mentions "cron", "cron jobs", "scheduled tasks", or "RightCron"
- The user asks to schedule, create, remove, or change a recurring task
- The user asks about cron run history or why a job failed

## How It Works

Cron specs are YAML files in `crons/`. The Rust runtime (`rightclaw bot`) polls this directory every 60 seconds and schedules jobs automatically. Creating, editing, or deleting a YAML file is all the agent needs to do — no API calls, no manual sync, no state tracking. The runtime handles lock files, log capture, and run history via the database.

## Creating a Cron Job

1. `mkdir -p crons` if the directory doesn't exist
2. Write a YAML spec to `crons/<job-name>.yaml`
3. Confirm to the user: "Job created. The runtime picks up new specs within ~60 seconds."

## Editing a Cron Job

1. Edit the YAML file with the new schedule, prompt, or other fields
2. Confirm: "Job updated. Changes take effect within ~60 seconds."

## Removing a Cron Job

1. Delete the YAML file: `rm crons/<job-name>.yaml`
2. Confirm: "Job removed. The runtime drops it within ~60 seconds."

## YAML Spec Format

Each cron job is a YAML file in the `crons/` directory. The filename (without `.yaml`) is the job name.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `schedule` | string | Yes | - | Standard 5-field cron expression (minute hour day-of-month month day-of-week). Evaluated in **UTC** by the Rust runtime. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g., `10m`, `1h`, `30m`). |
| `max_turns` | integer | No | - | Passed as `--max-turns` to limit Claude's execution turns. |
| `prompt` | string | Yes | - | The task prompt text that Claude executes when the cron fires. |

**Example specs:**

```yaml
# crons/deploy-check.yaml
schedule: "*/5 * * * *"
lock_ttl: 10m
max_turns: 5
prompt: "Check CI status for all open PRs, post comment if broken"
```

```yaml
# crons/morning-briefing.yaml
schedule: "0 9 * * 1-5"  # 09:00 UTC weekdays
lock_ttl: 30m
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
```

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

Returns the same fields as `cron_list_runs`. Returns a "not found" message for unknown IDs (not an error).

### Reading logs

The `log_path` field in each run record points to the log file with full stdout and stderr output from the subprocess. Read it directly:

```
cat <log_path>
```

No MCP tool is needed for log content — direct file access via bash.

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

1. **UTC schedules**: Cron expressions are evaluated in UTC by the Rust runtime. Write specs accordingly (e.g., `0 9 * * 1-5` fires at 09:00 UTC, not local time).
2. **60-second polling**: The runtime re-reads `crons/*.yaml` every 60 seconds. After creating, editing, or deleting a spec file, changes take effect within ~1 minute.
