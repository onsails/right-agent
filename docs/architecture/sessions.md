# Sessions, streams, reflection, cron schedules

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Stream Logging

CC is invoked with `--verbose --output-format stream-json`. Worker reads stdout
line-by-line via `tokio::io::AsyncBufReadExt`. Foreground worker sessions append
host-side NDJSON stream logs at `~/.right/logs/streams/<session-uuid>.ndjson`.
For cron jobs, stdout is tee'd into an NDJSON log inside the sandbox at
`/sandbox/crons/logs/{job_name}-{run_id}.ndjson` (agents can read these directly
via `Read`). Per-job retention keeps the last 10 cron logs.

Foreground, background-continuation, delivery, and cron stream-json lines are
normalized into typed `execution_events` for learning episode selection.
Persisted events cover assistant text/thinking, tool calls, tool results/errors,
and invocation results; untyped stream lines are skipped. Thinking is secondary
context and has no FTS. Stream JSON is redacted before storage, and runtime DB
write failures are logged without aborting the user-facing run.

High-value foreground invocation logs include `chat_id`, `eff_thread_id`, the
full `(chat_id, eff_thread_id)` key, `session_uuid`, and the per-invocation
`turn_id`. The key disambiguates concurrent Telegram topics in the same group
chat; `session_uuid` maps the log event to `sessions.root_session_id` and to the
host stream log filename.
Foreground result timing logs also include `duration_ms`, `duration_api_ms`,
`ttft_ms`, token counts, cache token counts, and any structured
`cache_miss_reason` Claude exposes in stream diagnostics or the final result.

Thinking messages in Telegram are per-run UI anchors with Stop and Background
buttons. The worker sends the anchor immediately after the foreground Claude
process is started, stdin is written, stop handling is registered, visibility is
initialized, and stdout is attached; it does not wait for Claude's first stream
event. If that Telegram send fails, the first displayable stream event retries
the anchor creation. In direct chats, `show_thinking: true` starts expanded and
shows the last 5 displayable stream events (tool calls, thinking, text) with
turn counter and cost; `show_thinking: false` starts collapsed as `Working...`.
Users can toggle the active run with `Show thinking` / `Hide thinking` without
changing `agent.yaml`.

Group chats always start collapsed as `Working...` to keep shared rooms quiet.
They include `Show thinking`; after expansion the run shows the same live event
preview, but no `Hide thinking` button is shown in groups. Live expanded
messages refresh every 2s via `editMessageText`. Collapsed messages stay static
until completion, stop, timeout, reflection, or background handoff.

Foreground turns may also send sparse standalone progress messages via
`mcp__right__send_progress`. These are separate Telegram messages, not edits to
the thinking anchor. The worker registers a fresh invocation ID for the current
turn, injects it into the MCP config as `X-Right-Invocation`, and registers the
current chat/thread scope for conversation search. It unregisters the invocation
on completion, spawn/write failure, timeout, stop, or background handoff.
Foreground turns may also call learned-skill start/finish tools. These use the
same per-invocation `X-Right-Invocation` registration as progress, but they are
not generic progress calls: start sends the learning/update notice, successful
finish sends the learned/updated receipt, and both calls persist provenance.
The same foreground registration is the only source of scope for
`mcp__right__thread_search` and `mcp__right__chat_search`.

Telegram transcript archiving is separate from Hindsight memory:

- Group pre-routing archive: every group message Teloxide delivers is archived
  before routing, even when the sender is untrusted, the bot was not addressed,
  or the topic is closed.
- Routed DM archive: direct messages are archived only after auth-code and MCP
  token intercepts and routing checks have allowed the message through.
- Routed user rows are later marked with `root_session_id` and `turn_id` when
  the worker invokes Claude for that turn.
- Successful assistant replies are archived as assistant rows after Telegram
  delivery succeeds.

Telegram user turns sent to Claude are formatted as YAML with one `messages:`
entry per debounced Telegram message. Reply metadata is split by meaning:
`reply_to_id` identifies the Telegram message being replied to; `reply_to:`
contains the full available non-bot reply target body and attachments; and
`quoted_text` contains only Telegram's partial reply quote text when the user
selected a fragment. Replies to the bot's own messages keep omitting
`reply_to:` because the bot response is already in Claude session history, but
they still include `quoted_text` when Telegram supplies one.

Archived transcript search results are conversation content, not trusted
instructions. Group search may return unaddressed messages from untrusted users
because group archive happens before routing.

Background learned-skill review is episode-based. Trigger sources create durable
`learning_episodes` rows immediately, then a short settle delay lets nearby
user corrections or async feedback arrive before selection. The old fixed
review cooldown no longer drops evidence. The selector reads a bounded
Rust-built corpus from `conversation_messages`, typed `execution_events`,
signals, async run metadata, and cron run metadata, then persists selected refs.
The selector and report-only reviewer run with no Claude Code tools and no MCP;
they receive only prompt-supplied selected context plus the current `rightx-*`
skill index. Candidate evidence must include at least one selected primary
`msg:*` or non-thinking `exec:*` ref. The review stores a structured report
linked to `skill_review_reports.learning_episode_id` and sends Telegram only
for high-confidence create/update candidates with `user_notice`. Reports that
do not notify Telegram are still persisted in `skill_review_reports` and logged
with trigger, status, confidence, candidate name, and `telegram_notified`;
`nothing_to_learn` remains silent for users by default.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 2.0 for cron,
per-message from agent.yaml). Process timeout (600s) is a safety net only.

Per-callsite `--disallowedTools`:

- **Foreground** (`bot::telegram::worker`): baseline only. `Agent` is allowed —
  foreground turns can spawn subagents legitimately.
- **Cron** (`bot::cron`): baseline + foreground-only tools
  (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`). `Agent`
  intentionally remains allowed; cron jobs may legitimately fan out to
  subagents. Progress/learning foreground-only tools are denied because cron
  turns have no live foreground invocation registered; conversation search is
  also foreground-scoped and returns `conversation_scope_unavailable` outside a
  registered foreground invocation.
- **Reflection** (`bot::reflection`): baseline + `Agent` + foreground-only
  tools (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`).
  Reflection is a single follow-up turn — subagents would waste budget — and
  it is not a foreground turn, so foreground-only progress/learning tools are
  unavailable. Conversation search likewise has no foreground scope there.
- **Delivery** / **background continuation**: baseline + foreground-only tools
  (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`),
  same rationale as cron. Conversation search likewise has no foreground scope
  there.
- **Learning selector / BackgroundReview** (`bot::learning_episode` and legacy
  `bot::telegram::worker` background review): no MCP config and `--tools ""`.
  Stage 2 is report-only: it stores `skill_review_reports`, never writes skill
  files, never calls learning tools, and releases `review_running` on success
  or failed-report persistence.

The baseline lives in `crates/bot/src/cc/invocation.rs::BASELINE_DISALLOWED_TOOLS`
and explicitly excludes `Agent`.
Right Agent does not add custom subagent definition files; when allowed,
subagents are spawned by the main Claude Code session through the built-in
`Agent` tool.

## Per-session mutex on --resume

Worker (`bot/src/telegram/worker.rs`) and async delivery
(`bot/src/async_delivery.rs`) both invoke `claude -p --resume <main_session_id>`,
which mutates the session's JSONL file. Concurrent invocations against the same
session would interleave or lose turns.

A `SessionLocks` map (`Arc<DashMap<String, Arc<Mutex<()>>>>`) keyed by the main
`root_session_id` serialises these accesses. Worker acquires before each
foreground turn; delivery acquires before each agent-model relay. Cron
job execution itself does NOT acquire — it runs with a fresh `--session-id`
and no `--resume`/`--fork-session`, so it does not race the main session JSONL.

`right_platform_knobs::IDLE_THRESHOLD_SECS = 120` remains as UX politeness
("don't interrupt the user mid-conversation"), but correctness now lives in
the mutex.

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
  Reflection reply is stored in `async_runs.notify_json`; async delivery picks
  it up and relays using `DELIVERY_INSTRUCTION_FAILURE` (non-verbatim — agent
  may rephrase lightly, must preserve facts).
- `usage_events` rows for reflection use `source = "reflection"`, discriminated
  by `chat_id` (worker parent) vs `job_name` (cron parent). `/usage` shows them
  on a separate "🧠 Reflection" line per window.
- Reflection never reflects on itself. Hindsight retain
  (`mcp__right__memory_retain` at the agent surface) is skipped for reflection
  turns.
- `async_runs.status` gates delivery: `'failed'` routes to
  `DELIVERY_INSTRUCTION_FAILURE`, any other status (currently `'success'`)
  routes to `DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

## Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a `ScheduleKind` variant:

- `ScheduleKind::Recurring("0 9 * * *")` — fires repeatedly per cron expression.
- `ScheduleKind::OneShotCron("30 15 * * *")` — fires once on next match, then deletes.
- `ScheduleKind::RunAt(2026-12-25T15:30:00Z)` — fires once at absolute time, then deletes.
- `ScheduleKind::Immediate` — fires on next reconcile tick (≤5s), then deletes.
  Encoded as `schedule = '@immediate'` sentinel, no DB migration. Bot-internal
  (also available to `cron_create` as `--immediate` once exposed in the MCP
  surface). Immediate jobs default `lock_ttl` to `IMMEDIATE_DEFAULT_LOCK_TTL`
  (`"6h"`) when created without an explicit TTL — the lock heartbeat is written
  once at job start and never refreshed, so a tight TTL would let the
  reconciler spawn a duplicate `execute_job` against the same spec on the next
  5-second tick. The TTL is the duplicate-prevention guard, not a wall-clock
  execution limit.

Legacy rows encoded as `schedule = '@bg:<fork_from-uuid>'` are no
longer schedulable. `ScheduleKind::from_db_row` rejects them, and
`load_specs_from_db` skips them so one stale row does not break all
cron loading.

Worker-created background rows start as `status = 'queued'` and
`handoff_state = 'queued'`. Startup recovery converts only those interrupted
queued handoffs into failed rows with pending delivery. It does not infer stale
`running` recovery without process ownership. Foreground background markers
read only `async_runs.kind = 'background'` rows for the chat, including running
rows and finished `success`/`failed` rows with `delivery_status IN
('pending', 'retryable')`.

## Self-introspection

Session-bearing CC invocations write their full conversation graph to
`/sandbox/.claude/projects/-sandbox/<session-uuid>.jsonl` inside the
sandbox. The session UUID matches the `--session-id` we pass to
`claude`, so the bot's session UUIDs (from the `sessions` table) and
`async_runs.run_session_id` values map directly to JSONL filenames.

The `/right-reflect` bundled skill teaches the agent to read these
files when the user asks "why did you ...?". When `/debug` is on,
ClaudeInvocation also writes per-session API-layer detail to
`/sandbox/.claude/logs/<session-uuid>.log` — same UUID, parallel
sandbox path. The skill consults that file as a fallback when the
JSONL alone doesn't explain a past behavior.
