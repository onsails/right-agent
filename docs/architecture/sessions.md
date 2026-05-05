# Sessions, streams, reflection, cron schedules

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. For cron jobs, stdout is tee'd into
an NDJSON log inside the sandbox at `/sandbox/crons/logs/{job_name}-{run_id}.ndjson`
(agents can read these directly via `Read`). Per-job retention keeps the last 10 logs.
Worker sessions do not write stream logs.

When `show_thinking: true` (default), a live thinking message in Telegram shows
the last 5 events (tool calls, text) with turn counter and cost. Updated every 2s
via `editMessageText`. Stays in chat after completion.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 2.0 for cron,
per-message from agent.yaml). Cron jobs disable `Agent` tool to prevent budget waste on
subagent branches. Process timeout (600s) is a safety net only.

## Per-session mutex on --resume

Worker (`bot/src/telegram/worker.rs`) and cron delivery
(`bot/src/cron_delivery.rs`) both invoke `claude -p --resume <main_session_id>`,
which mutates the session's JSONL file. Concurrent invocations against the same
session would interleave or lose turns.

A `SessionLocks` map (`Arc<DashMap<String, Arc<Mutex<()>>>>`) keyed by the main
`root_session_id` serialises these accesses. Worker acquires before each
foreground turn; delivery acquires before each Haiku-relayed delivery. Cron
job execution itself does NOT acquire — it runs `--fork-session` against a new
session ID and does not race the main session JSONL.

`IDLE_THRESHOLD_SECS = 180` remains as UX politeness ("don't interrupt the
user mid-conversation"), but correctness now lives in the mutex.

Sweep: a periodic task in `lib.rs` (every hour) drops entries whose Arc has no
external strong references — protects against unbounded growth on long-lived
agents.

## Reflection Primitive

`crates/bot/src/reflection.rs` exposes `reflect_on_failure(ctx) -> Result<String, ReflectionError>`.
On CC invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a human-friendly
summary of the failure instead of the raw ring-buffer dump.

- Worker uses `ReflectionLimits::WORKER` (3 turns, $0.20, 90s process timeout).
  Reflection reply is sent to Telegram directly; on reflection failure, the
  caller falls back to the raw error message.
- Cron uses `ReflectionLimits::CRON` (5 turns, $0.40, 180s process timeout).
  Reflection reply is stored in `cron_runs.notify_json`; `cron_delivery` picks
  it up and relays using `DELIVERY_INSTRUCTION_FAILURE` (non-verbatim — agent
  may rephrase lightly, must preserve facts).
- `usage_events` rows for reflection use `source = "reflection"`, discriminated
  by `chat_id` (worker parent) vs `job_name` (cron parent). `/usage` shows them
  on a separate "🧠 Reflection" line per window.
- Reflection never reflects on itself. Hindsight `memory_retain` is skipped for
  reflection turns.
- `cron_runs.status` gates delivery: `'failed'` routes to
  `DELIVERY_INSTRUCTION_FAILURE`, any other status (currently `'success'`)
  routes to `DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

## Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a `ScheduleKind` variant:

- `ScheduleKind::Recurring("0 9 * * *")` — fires repeatedly per cron expression.
- `ScheduleKind::OneShotCron("30 15 * * *")` — fires once on next match, then deletes.
- `ScheduleKind::RunAt(2026-12-25T15:30:00Z)` — fires once at absolute time, then deletes.
- `ScheduleKind::Immediate` — fires on next reconcile tick (≤5s), then deletes.
  Encoded as `schedule = '@immediate'` sentinel, no DB migration. Used by the
  bot for background-continuation jobs (also available to `cron_create` as
  `--immediate` once exposed in the MCP surface). `insert_immediate_cron`
  defaults `lock_ttl` to `IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`) when the caller
  passes none — bg-continuation turns can legitimately run for hours, and the
  lock heartbeat is written once at job start and never refreshed, so a tight
  TTL would let the reconciler spawn a duplicate `execute_job` against the
  same spec on the next 5-second tick. The TTL is the duplicate-prevention
  guard, not a wall-clock execution limit.
