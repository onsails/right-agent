---
name: rightcron
description: >-
  Manages scheduled cron jobs for this RightClaw agent. On EVERY session startup,
  schedules a periodic reconciler and recovers persisted jobs. Handles user requests
  to create, remove, and change recurring tasks. Use when the user mentions cron jobs,
  scheduled tasks, RightCron, recurring tasks, or at session start.
version: 0.2.0
---

# /rightcron -- RightCron Cron Manager

You are the RightCron cron manager for this RightClaw agent.

## When to Activate

Activate this skill when:
- **Session startup** — ALWAYS run bootstrap (see below) on every new session
- The user mentions "cron", "cron jobs", "scheduled tasks", or "RightCron"
- The user asks to schedule, create, remove, or change a recurring task
- The user asks to "sync cron jobs", "reconcile schedules", or "check scheduled tasks"

## Bootstrap (MUST run on every session start)

On EVERY session start, regardless of whether `crons/` exists:

1. **Create `crons/` directory** if it doesn't exist: `mkdir -p crons`
2. **Schedule the reconciler job** via CronCreate:
   - Schedule: `*/5 * * * *` (every 5 minutes)
   - Prompt: `Run /rightcron reconcile` (this triggers the reconciliation algorithm below)
   - This job is session-scoped (auto-expires after 3 days) but RightCron re-creates it on every startup
   > Call CronCreate directly in this turn — do NOT use the Agent tool.
3. **Run initial reconciliation** — execute the reconciliation algorithm (Steps 1-6) to recover any persisted jobs from `crons/*.yaml`
4. Report silently — do NOT message the user about bootstrap unless there were recovered jobs

The reconciler job ensures that YAML specs created by other tools or modified externally are picked up within 5 minutes.

## Conversational Job Creation

When the user asks to create a scheduled task (e.g., "schedule X every 5 minutes"):

1. **Create the `crons/` directory** if it doesn't exist: `mkdir -p crons`
2. **Write a YAML spec file** to `crons/<job-name>.yaml` with the schedule and prompt
3. **Run the reconciliation algorithm** (Step 1-6 below) to register the job via CronCreate
4. **Report** the job as created with "Persistent — survives agent restarts via RightCron"

**CRITICAL:** NEVER call CronCreate directly without first writing a YAML spec file. The YAML spec is the source of truth. Without it, the job cannot be recovered after agent restart.

When the user asks to remove a job:

1. **Delete the YAML spec file** from `crons/<job-name>.yaml`
2. **Run reconciliation** — the orphan detection (Step 4.4) will delete the live job
3. **Report** the job as removed

When the user asks to change a job's schedule or prompt:

1. **Edit the YAML spec file** with the new values
2. **Run reconciliation** — change detection (Step 4.2) will recreate the job
3. **Report** the updated schedule/prompt

## YAML Spec Format

Each cron job is defined as a YAML file in the `crons/` directory. The filename (without `.yaml` extension) is the job name.

**Fields:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `schedule` | string | Yes | - | Standard 5-field cron expression (minute hour day-of-month month day-of-week). Claude Code interprets cron in LOCAL timezone. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g., `10m`, `1h`, `30m`). |
| `max_turns` | integer | No | - | Passed as `--max-turns` to prevent runaway sessions. |
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
schedule: "0 9 * * 1-5"
lock_ttl: 30m
prompt: "Gather open PRs, failing tests, pending reviews. Post summary to Slack."
```

## Reconciliation Algorithm

> **CRITICAL: NEVER use the Agent tool for CronCreate, CronDelete, or CronList calls. These
> tools are only available in the main conversation thread. Any delegation to a background
> agent will silently fail — CronCreate will not be found by the background agent's ToolSearch.
> All cron tool calls MUST happen directly in this conversation turn.**

Run the following steps in order. This algorithm is idempotent -- safe to run multiple times.

### Step A: Compute diff (CHECK)

Read all inputs and compute what needs to change. No CronCreate or CronDelete calls in this step.

#### Step 1: Read desired state

Read all `crons/*.yaml` files. Parse each into a map keyed by job name (filename without `.yaml`):

```
desired = {
  "deploy-check": { schedule: "*/5 * * * *", lock_ttl: "10m", max_turns: 5, prompt: "..." },
  "morning-briefing": { schedule: "0 9 * * 1-5", lock_ttl: "30m", prompt: "..." }
}
```

If `crons/` does not exist or contains no `.yaml` files, report "No cron specs found" and stop.

#### Step 2: Read actual state

Call `CronList` to get all live cron jobs. This returns job IDs, schedules, and prompts for every active task. Note: CronList is a READ-ONLY call, allowed in CHECK.

#### Step 3: Read tracked state

Read `crons/state.json`. If the file does not exist, start with an empty map `{}`.

Format of `crons/state.json`:

```json
{
  "deploy-check": {
    "job_id": "4e9fed67",
    "schedule": "*/5 * * * *",
    "prompt_hash": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
  },
  "morning-briefing": {
    "job_id": "06c25e84",
    "schedule": "0 9 * * 1-5",
    "prompt_hash": "f1e2d3c4b5a6f7e8d9c0b1a2f3e4d5c6b7a8f9e0d1c2b3a4f5e6d7c8b9a0f1e2"
  }
}
```

The `prompt_hash` is a SHA-256 hash of the full prompt text (including the lock guard wrapper). Compute it with:
```bash
echo -n "<prompt_text>" | sha256sum | awk '{print $1}'
```

#### Diff computation

After reading all three states, compute the diff:
- **New jobs**: in desired state, not in tracked state
- **Changed jobs**: in tracked state but schedule or prompt_hash differs from desired state
- **Unchanged jobs**: in tracked state, values match desired state
- **Orphaned jobs**: in tracked state, not in desired state

### Step B: Apply changes (RECONCILE)

Based on the diff computed in Step A, make the changes. All CronCreate and CronDelete calls happen
here, directly in this conversation turn.

#### Step 4: Reconcile

**Check task count first.** Before creating any new jobs, count existing tasks from CronList. Claude Code has a hard limit of 50 tasks per session. If the total (existing + new) would exceed 50, warn the user and stop.

For each entry in **desired state**:

1. **Not in tracked state** (new job):
   - Construct the wrapped prompt with lock guard (see Lock Guard Wrapper below)
   - Call `CronCreate` with the schedule and wrapped prompt
   - Record in state.json: `job_id`, `schedule`, `prompt_hash` (hash of the wrapped prompt)

2. **In tracked state but changed** (schedule differs OR prompt_hash differs):
   - Compute the hash of the current wrapped prompt
   - Compare `schedule` and `prompt_hash` with tracked values
   - If either differs:
     - Call `CronDelete` with the old `job_id`
     - Construct the new wrapped prompt with lock guard
     - Call `CronCreate` with the new schedule and wrapped prompt
     - Update state.json with new `job_id`, `schedule`, `prompt_hash`

3. **In tracked state and unchanged**:
   - Verify the job still exists in actual state (CronList output) by matching `job_id`
   - If missing (expired after 3 days -- this is expected): recreate via `CronCreate` with the same wrapped prompt, update `job_id` in state.json
   - If present: skip (no action needed)

For each entry in **tracked state** NOT in desired state:

4. **Orphaned job** (spec file was deleted):
   - Call `CronDelete` with the `job_id`
   - Remove the entry from state.json
   - Delete the lock file at `crons/.locks/<name>.json` if it exists

#### Step 5: Write updated state.json

Write the reconciled state map back to `crons/state.json` as formatted JSON.

#### Step 6: Report

Summarize the actions taken:

> RightCron reconciliation complete:
> - Created: N jobs
> - Deleted: N orphaned jobs
> - Recreated: N expired jobs
> - Updated: N changed jobs
> - Unchanged: N jobs

If no actions were taken: "All cron jobs are in sync."

## Lock Guard Wrapper

When creating a cron job via `CronCreate`, the prompt MUST be wrapped with lock guard logic. Construct the full wrapped prompt string before calling `CronCreate`.

The wrapped prompt follows this pattern:

```
Before executing the task, check the lock file:

1. Check if crons/.locks/<name>.json exists.
   - If it exists, read the heartbeat timestamp.
   - Parse the heartbeat as UTC ISO 8601.
   - If the heartbeat is less than <lock_ttl> ago (compare with current UTC time via `date -u`): skip this run -- previous execution is still active. Report "Skipped <name>: previous run still active" and stop.
   - If the heartbeat is more than <lock_ttl> ago: the lock is stale. Delete the lock file and continue.
   - If the file does not exist: continue.

2. Create the lock file:
   ```bash
   mkdir -p crons/.locks
   echo '{"heartbeat":"'$(date -u +"%Y-%m-%dT%H:%M:%SZ")'"}' > crons/.locks/<name>.json
   ```

3. Execute the task:
   <original_prompt>

4. On completion (success or failure), delete the lock file:
   ```bash
   rm -f crons/.locks/<name>.json
   ```
```

Replace `<name>` with the job name, `<lock_ttl>` with the spec's lock_ttl value (default `30m`), and `<original_prompt>` with the prompt from the YAML spec.

## Lock File Format

Lock files are stored at `crons/.locks/<name>.json`.

```json
{"heartbeat": "2026-03-22T10:05:00Z"}
```

All lock file timestamps MUST use UTC ISO 8601 format with the `Z` suffix. Generate timestamps with:
```bash
date -u +"%Y-%m-%dT%H:%M:%SZ"
```

## File Layout

```
crons/
  deploy-check.yaml        # Cron spec (source-controlled)
  morning-briefing.yaml    # Cron spec (source-controlled)
  state.json               # Job ID mapping (gitignored -- runtime artifact)
  .locks/                  # Lock files (gitignored -- runtime artifact)
    deploy-check.json
    morning-briefing.json
```

`state.json` and `.locks/` are runtime artifacts, not source-controlled. Add to `.gitignore`:
```
crons/state.json
crons/.locks/
```

## Constraints and Pitfalls

1. **50-task limit:** Claude Code allows max 50 scheduled tasks per session. Count existing tasks before creating new ones. If approaching the limit, warn the user.

2. **3-day auto-expiry:** All cron jobs auto-expire after 3 days. RightCron handles this automatically by recreating expired jobs from the YAML specs on each reconciliation run. This is expected behavior, not an error.

3. **Timezone:** Cron schedules fire in LOCAL timezone (this is how Claude Code interprets 5-field cron expressions). Lock file timestamps use UTC. Be explicit: use `date -u` for all lock file operations.

4. **TOCTOU on lock files:** The check-then-create pattern on lock files has a theoretical race condition. This is acceptable because Claude Code is single-threaded -- scheduled prompts fire between turns, never concurrently. The lock files protect against overlapping runs of the SAME job (slow previous run + new trigger), not true concurrent access.

5. **No CronUpdate:** Claude Code does not provide a CronUpdate tool. To change a job's schedule or prompt, delete the old job and create a new one.

## Important Rules

1. All lock file timestamps MUST use UTC ISO 8601 format with the `Z` suffix.
2. `state.json` and `crons/.locks/` are runtime artifacts -- not source-controlled.
3. RightCron is idempotent. Running it multiple times with no spec changes produces no side effects.
4. Always wrap prompts with the lock guard before passing to CronCreate.
5. Use `sha256sum` for prompt hash computation to detect changes.
6. Recreate expired jobs silently -- 3-day expiry is normal, not an error condition.
7. **Jobs are NOT session-only.** RightCron recovers all jobs from YAML specs on agent restart. When reporting job creation, say "Persistent — survives agent restarts via RightCron." Do NOT say "session-only".
8. **Always use the remote channel for output.** Cron job prompts that communicate with the user MUST use the `reply` MCP tool (remote channel) — never console output. The agent runs as a daemon with no terminal access. Include "Reply to the user via the remote channel (use the reply MCP tool)" in every cron job prompt that produces user-facing output.
