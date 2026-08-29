---
name: right-cron
description: >-
  Manages cron jobs for this Right Agent agent via MCP tools. Creates, updates,
  and deletes cron specs stored in the agent database. The Rust runtime handles
  scheduling and execution automatically. Use when the user mentions cron
  jobs, scheduled tasks, reminders, one-shot tasks, or recurring tasks.
version: 3.7.0
---

# /right-cron -- Cron Job Manager

## When to Activate

Activate this skill when:
- The user mentions "cron", "cron jobs", "scheduled tasks", "reminders", or "Right Cron"
- The user asks to schedule, create, remove, or change a recurring or one-shot task
- The user asks to run something at a specific time or after a delay
- The user asks about cron run history or why a job failed
- **You (the agent) need to return to a task without a new user message** —
  e.g. retrying a transient upstream failure (502/timeout), polling a
  long-running external job, or honoring a "let me know when X" request.
  Cron is the only deferred-action mechanism available to you; if you'd
  otherwise say "I'll try again later", create a one-shot cron instead of
  promising it.

## How It Works

Cron specs are stored in the agent database. The Rust runtime reconciles specs continuously and schedules jobs automatically. Use MCP tools to manage specs — no file creation needed.

## Creating a Cron Job

**Minimum delivery latency.** A cron notification is held in the delivery
queue until the chat has been idle for **{{ idle_threshold_min }} minutes**
(UX-politeness gate — see "Triggering a Cron Job Manually" below, and
`notify: true` to bypass it). Therefore:

- Never promise the user a reminder sooner than {{ idle_threshold_min }}
  minutes from now. The scheduled `run_at` can be sooner, but the user will
  not actually see the message until the chat has been quiet for
  {{ idle_threshold_min }} minutes.
- If the user requests a reminder in less than {{ idle_threshold_min }}
  minutes, say so plainly: "I can schedule it, but it won't reach you until
  ~{{ idle_threshold_min }} minutes after you stop typing — want me to set it
  for then instead?" Then act on their choice.

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

## Writing Cron Prompts

The cron runs as a separate, non-interactive session — its only
delivery channel is its structured output. Phrase the `prompt:` as a
**task that produces text**, not as an imperative messaging action.
Imperative verbs like "send", "tag", "notify", "ping" prime the cron
agent to look for an external messaging tool.

Cron `delivery.content` is RichContent (`{"text":"literal"}` or typed
`{"blocks":[...]}`), never a Markdown string.

| User said                            | Store as                                                  |
|--------------------------------------|-----------------------------------------------------------|
| "Tag @bob with a reminder about X"   | "Output a reminder about X, mentioning @bob"              |
| "Send a message to @alice at 9am"    | "Output a heads-up about X, addressed to @alice"          |
| "Ping me when Y happens"             | "Check Y. If it happened, output a notification about it" |
| "Notify the channel about Z"         | "Output a notification about Z"                           |

@username is fine as plain text — it ends up in the delivered
message and Telegram renders it as a mention. Don't strip the user's
content or schedule; only rephrase the delivery-imperative verbs.

### What, not how

A cron `prompt:` states the **goal** — the outcome you want — and trusts your
skills to supply the **procedure**. Do not inline brittle step-by-step "how"
into the prompt: it can't be improved centrally and rots as tools change. When
a cron's procedure is non-trivial, capture it as a `rightx-*` skill (see
right-learn-skill) before creating the cron, then write the cron's "what". At
fire time the cron loads the skill and executes.

## Linking Skills to a Cron

A cron can have one or more `rightx-*` skills **linked** to it. At fire time the
runtime names the job's live linked skills as authoritative, so the cron pulls
them deterministically instead of relying on description-matching.

- **Automatic:** any skill a recurring cron's own runs create or patch is linked
  to that cron automatically. You do not need to link those by hand.
- **At creation:** pass `skill_names` to `cron_create` to link existing skills up
  front (capture the skill first, then create the cron that uses it).
- **For existing crons:** `mcp__right__cron_link_skill(job_name, skill_names=[...])`
  links several at once; `mcp__right__cron_unlink_skill(job_name, skill_names=[...])`
  removes them. (A skill the cron re-learns may be auto-linked again.)

`mcp__right__cron_list` shows each job's `linked_skills`.

## Choosing the Model

The session that creates a cron picks that cron's model by judging the task's
complexity — the runtime never decides for you. Set `model:` on `cron_create`
(and `cron_update`) deliberately:

- **`haiku`** — trivial: one request or tool call plus mechanical formatting of
  the result (fetch a page, extract a field, report it). No reasoning, no
  multi-step decisions.
- **`sonnet`** — mechanical multi-step with light judgment: health checks,
  summaries, scheduled briefings, status polls. **This is the right default for
  most crons.**
- **`opus`** — genuinely complex: multi-source research, nuanced analysis,
  anything you'd want your strongest model for.

Omit `model` only when you deliberately want the cron to track the agent's
current `/model`. Otherwise set it explicitly — a mechanical cron left on an
Opus global wastes budget and latency.

To change a cron's model later: `mcp__right__cron_update(job_name: "...", model: "sonnet")`.
Pass `model: null` to clear it back to inheriting the agent's `/model`.

## Editing a Cron Job

Use the `mcp__right__cron_update` MCP tool. Only pass the fields you want to change — unspecified fields keep their current values.

**Evolve the prompt toward its skills.** Before `cron_update`-ing a prompt, check
the job's `linked_skills` (`cron_list`). If the prompt still spells out "how" that
a linked skill now covers, slim the prompt to the "what" and rely on the link —
migrating a fat cron prompt toward a thin goal + skill references. Tell the user
what you simplified; never rewrite silently.

```
# Change only the prompt
mcp__right__cron_update(job_name: "health-check", prompt: "New check prompt")

# Change only the schedule
mcp__right__cron_update(job_name: "health-check", schedule: "43 */4 * * *")

# Switch from recurring to one-shot run_at
mcp__right__cron_update(job_name: "health-check", run_at: "2026-04-16T10:00:00Z")
```

Setting `schedule` clears `run_at`. Setting `run_at` clears `schedule` and forces `recurring=false`.

## Removing a Cron Job

Use the `mcp__right__cron_delete` MCP tool:

```
mcp__right__cron_delete(job_name: "health-check")
```

## Triggering a Cron Job Manually

Use the `mcp__right__cron_trigger` MCP tool to run a job immediately:

```
mcp__right__cron_trigger(job_name: "health-check")               # conditional delivery
mcp__right__cron_trigger(job_name: "health-check", notify: true) # force the result to the user
```

The job is queued for the cron engine. Lock check still applies — if the job is currently running, the trigger is skipped.

**`notify` decides whether the user hears the result** — it is the single most
important argument on this tool:

- omitted / `notify: false` (default) — *conditional*. The job's own output
  decides: `delivery.kind = "notify"` sends `delivery.content`; `delivery.kind =
  "silent"` sends nothing (records `delivery.reason`). Any notification is also
  held until the chat has been idle for **{{ idle_threshold_min }} minutes**
  (UX-politeness gate). A job that decides to stay silent (e.g. "nothing
  changed", "skipped today") delivers nothing at all.
- `notify: true` — *forced*. Overrides a silent decision and skips the idle
  gate, so the user reliably receives the result promptly — even from a job that
  would otherwise go silent.

**When the user says "run X and tell me the result" — or anything whose outcome
must reach them — pass `notify: true`.** Don't fall back to "I'll check
`cron_list_runs` later"; that is a diagnostic tool, not a delivery mechanism.

**One-off tweaks.** To adjust a single run without changing the stored job, pass
`extra_instruction` on `mcp__right__cron_trigger`. It is prepended to this run
only and leaves the spec's `prompt` untouched — use it instead of
`mcp__right__cron_update`, which mutates the stored prompt for every future run.

**Report back to this chat.** Attach a `then` to surface the result here:

```
mcp__right__cron_trigger(
  job_name: "health-check",
  then: { run_on: "always", notify: true, instruction: "summarize how it went" }
)
```

A `then` with no `target_chat_id` delivers back to the chat you triggered from.

## Chaining Jobs and Multi-Step Pipelines

`notify: true` replaces the old "watch it with a second cron" habit. Do NOT
create a watcher, poller, or finish-notifier cron just to check a job and report
its outcome — that proliferates throwaway crons and routinely drops the notify
intent. To surface a job's result, trigger it with `notify: true`.

**Sequencing (run B only after A finishes).** Attach a `then` to A's
`mcp__right__cron_trigger`. It is runtime-guaranteed: when A reaches the terminal
state matching `run_on` (`success` \| `failure` \| `always`), the runtime resumes
(forks) A's session and runs `then.instruction`, so B sees what A actually did —
no second watcher cron, no lost notify intent.

```
mcp__right__cron_trigger(
  job_name: "build-report",
  then: { run_on: "success", notify: true, instruction: "publish the report you just built" }
)
```

`run_on` is required. `then.notify: true` forces the follow-up's report to the
user. Delivery defaults to the chat you triggered from; set `then.target_chat_id`
to override.

Prefer the fewest jobs that work: one trigger with a `then` beats a chain of
probe + retry + notifier crons.

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
| `skill_names` | string[] | No | - | `cron_create` only. rightx-* skills to link at creation; pulled deterministically at fire time. Skills must already exist. |
| `lock_ttl` | string | No | `30m` | Duration after which a lock is considered stale (e.g. `10m`, `1h`). |
| `max_budget_usd` | number | No | `2.0` | Maximum dollar spend per invocation. Claude stops gracefully when budget is reached. |
| `model` | enum | No | inherit | `haiku` \| `sonnet` \| `opus`. Picks the model for this cron by complexity (see "Choosing the Model"). Omit to inherit the agent's current `/model`. |
| `target_chat_id` | integer | No | - | Telegram chat ID to deliver cron notifications to. See guidance below. |
| `target_thread_id` | integer | No | - | Message thread ID within a supergroup topic. Only relevant when `target_chat_id` is a supergroup with topics enabled. |

### `cron_trigger`-only parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `extra_instruction` | string | No | - | Prepended to THIS run only; does not change the stored `prompt`. |
| `then` | object | No | - | Runtime-guaranteed follow-up that resumes (forks) this run's session after it finishes. Fields below. |
| `then.instruction` | string | Yes | - | The follow-up task. It can reference what the run just did. |
| `then.run_on` | enum | Yes | - | `success` \| `failure` \| `always`. When the follow-up fires relative to this run's outcome. |
| `then.notify` | boolean | No | `false` | Force the follow-up's report to the user (skip silent + idle gate). |
| `then.target_chat_id` | integer | No | origin | Override the follow-up's delivery chat. Defaults to the chat the trigger was issued from. |
| `then.target_thread_id` | integer | No | - | Message thread ID for the follow-up's delivery chat. |

## Delivery Target

**Always pass `target_chat_id`** when creating a cron job in response to a user request. Set it to the `chat.id` from the incoming message YAML — this makes the cron deliver back to the same chat where the user asked for it. The runtime rejects any chat ID not in the agent's allowlist.

For supergroup topics, also pass `target_thread_id` from the message's `chat.topic_id` if the user wants notifications in that specific topic thread.

To change a cron's delivery destination, call `mcp__right__cron_update` with the new `target_chat_id` (and optionally `target_thread_id`). Only deviate from the current chat if the user explicitly requests a different destination.

Example — creating a recurring job with delivery target:

```
mcp__right__cron_create(
  job_name: "morning-briefing",
  schedule: "17 9 * * 1-5",
  prompt: "...",
  target_chat_id: 123456789
)
```

## One-Shot Job Behavior

One-shot jobs (both `run_at` and `recurring: false`) auto-delete from `cron_specs` after execution.
- `cron_list` shows pending one-shot specs until they fire
- `cron_list_runs` shows execution history (runs are preserved even after spec deletion)
- If the bot was offline when `run_at` passed, the job fires on the next startup

### Schedule Guidelines

**Never silently fire at minute :00 or :30.** Peak minutes cluster with other automated jobs and spike API rate limits. This covers literal `0`/`30` AND step expressions that hit those minutes: `*/30`, `*/15`, `*/10`, `*/5`. The `cron_create` / `cron_update` tools also enforce this — they always return a `Warning:` line when the chosen schedule hits a peak minute.

If the user asks for a round interval (e.g. "every 30 minutes", "every hour at :00"), offset the minute field and tell them: `17,47 * * * *` for half-hourly, `43 * * * *` for hourly. Only use a peak minute when the user EXPLICITLY insists on the exact round time.

## Checking Run History

Use the `right` MCP server tools to check cron job execution history.

### mcp__right__cron_list_runs

Returns recent runs sorted by `started_at` descending.

Parameters:
- `job_name` (optional string) — filter by job name; omit to return all jobs
- `limit` (optional integer) — max runs to return; default 20

Each run record contains: `id`, `job_name`, `started_at`, `finished_at`, `exit_code`, `status`, `log_path`, `run_note`, `delivery`, `delivered_at`, `delivery_status`

**Delivery diagnostics:**
- `delivery_status`: lifecycle state — `none` (CC decided nothing to report), `pending` (awaiting delivery), `retryable` (delivery failed and will retry), `delivered` (sent to Telegram), `superseded` (newer run replaced this one), `failed` (delivery gave up after retries)
- `delivery`: structured decision; `delivery.kind = "notify"` carries user-facing `delivery.content`, while `delivery.kind = "silent"` carries `delivery.reason`
- `run_note`: technical run history/debug metadata; never delivered to Telegram
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

When the user asks "why wasn't I notified?", check `delivery_status` and `delivery`:

```
1. mcp__right__cron_list_runs(job_name="github-tracker", limit=5)
   -> Check delivery_status for each run:
      - "none" + delivery.kind = "silent" → CC decided nothing to report, delivery.reason explains why. If the user wanted the result regardless, re-run with `cron_trigger(job_name=..., notify=true)` to force it through.
      - "pending" → notification waiting for chat idle ({{ idle_threshold_min }} min threshold)
      - "retryable" → delivery failed and will retry
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
2. **Continuous reconciliation**: The runtime reconciles specs continuously; create/update/delete take effect promptly. Don't quote a specific number of seconds to the user — the polling interval is an internal detail.
3. **Manual triggers**: `mcp__right__cron_trigger` queues the job for the next reconciliation pass. If the job is locked (still running from a previous invocation), the trigger is skipped.
4. **Conditional delivery**: A successful run does not guarantee a Telegram message. The cron's structured output decides via `delivery.kind`: `"notify"` sends `delivery.content`, while `"silent"` records `delivery.reason`. Pending notifications wait for {{ idle_threshold_min }} minutes of chat idle before they are sent. `cron_trigger(..., notify=true)` overrides both — it forces a silent run to deliver and skips the idle wait.
