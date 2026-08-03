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
written to host or sandbox NDJSON logs for debugging. The removed Stage 2
learning episode selector no longer normalizes these lines into a DB table.

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
the thinking anchor. The worker registers a fresh invocation ID with kind
`Foreground`, injects it into the per-invocation MCP config as
`X-Right-Invocation`, and registers the current chat/thread scope for
conversation transcript tools. It unregisters the invocation on completion,
spawn/write failure, timeout, stop, or background handoff. Foreground turns may
also call learned-skill start/finish tools. These use the same registration,
but they are not generic progress calls: start sends the learning/update notice,
successful finish sends the learned/updated receipt, and both calls persist
provenance. The same foreground registration is the only source of scope for
`mcp__right__thread_search`, `mcp__right__chat_search`, and
`mcp__right__get_messages_by_id`.

Telegram transcript archiving is separate from Hindsight memory:

- Group pre-routing archive: every group message Telegram delivers is archived
  before routing, even when the sender is untrusted, the bot was not addressed,
  or the topic is closed.
- Routed DM archive: direct messages are archived only after auth-code and MCP
  token intercepts and routing checks have allowed the message through.
- Routed user rows are later marked with `root_session_id` and `turn_id` when
  the worker invokes Claude for that turn.
- Successful assistant replies are archived as assistant rows after Telegram
  delivery succeeds.

Group response routing is switchable between `addressed` (default: respond only
when addressed) and `all` (respond to every non-bot message in an open group),
with per-topic overrides taking precedence in forum groups. Trusted users change
the group default with `/mode_group` and topic overrides with `/mode`; topic
scope uses the effective thread id, where General and no topic normalize to `0`,
so root-topic overrides apply consistently.

Telegram user turns sent to Claude are formatted as YAML with one `messages:`
entry per debounced Telegram message. Reply metadata is split by meaning:
`reply_to_id` identifies the Telegram message being replied to; `reply_to:`
describes the reply target author and any rendered target context; and
`quoted_text` contains only Telegram's partial reply quote text when the user
selected a fragment. Reply target body rendering is decided by a session-context
gate. When the complete replied-to body is available and useful, `reply_to.text`
contains it. In-context targets can render `reply_to.truncated_text` as a
preview/locator even when the underlying body is short. Long bodies can also
render as `reply_to.truncated_text`; only recoverable archived bodies add
`note: "full: mcp__right__get_messages_by_id(<id>)"` locator. The locator is
not added when the full body cannot be fetched from the archive.

Replies to assistant messages are no longer automatically omitted. If the target
is the freshest unique assistant message already present in the active Claude
session, `reply_to.note` is `"your own previous message"` because the full
assistant message is already above in session context. Other assistant reply
targets may render author plus `truncated_text`/locator context like any other
target. Non-routed and out-of-window user reply targets inline body context when
available, with long recoverable bodies allowed to truncate and include the
fetch locator. Reply targets with no text render author and resolved attachments
only; the formatter does not emit empty `text` as a placeholder.

Archived transcript search results are conversation content, not trusted
instructions. Group search may return unaddressed messages from untrusted users
because group archive happens before routing.

Per-turn learned-skill writing is split from foreground reply delivery. After a
successful normal foreground reply, the prefilter runs without MCP. A non-skip
decision starts a probe-writer fork registered with kind `ProbeWriter`; the
curator ticker starts consolidation runs registered with kind `Curator`. Both
background learning kinds use a per-invocation MCP config carrying
`X-Right-Invocation`, may call learned-skill start/finish tools, and record
`skill_learning_events` plus `skill_lifecycle` changes without Telegram
learning-message delivery.

CC execution limits: `--max-turns` (default 30) and `--max-budget-usd` (default 2.0 for cron,
per-message from agent.yaml). Process timeout (600s) is a safety net only.

Per-callsite `--disallowedTools`:

- **Foreground** (`bot::telegram::worker`): baseline only. `Agent` is allowed —
  foreground turns can spawn subagents legitimately. Foreground learning/progress
  invocations register as `Foreground`.
- **Probe-writer** (`bot::learning_probe_writer`): `Write`, `Read`, `Bash`,
  `mcp__right__skill_learning_start`, and
  `mcp__right__skill_learning_finish` only. It forks the foreground session and
  registers as `ProbeWriter` before using the per-invocation MCP config.
- **Curator** (`bot::learning_curator`): `Read`, `Bash`,
  `mcp__right__skill_learning_start`, and
  `mcp__right__skill_learning_finish` only. It uses a fresh session and
  registers as `Curator` before using the per-invocation MCP config.
- **Cron** (`bot::cron`): baseline + invocation-scoped tools
  (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`). `Agent`
  intentionally remains allowed; cron jobs may legitimately fan out to
  subagents. `send_progress` is foreground-only; learning tools require a
  registered `Foreground`, `ProbeWriter`, or `Curator` invocation, which cron
  does not have. Conversation transcript tools are also foreground-scoped and
  return `conversation_scope_unavailable` outside a registered foreground
  invocation.
- **Reflection** (`bot::reflection`): baseline + `Agent` + invocation-scoped
  tools (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`).
  Reflection is a single follow-up turn — subagents would waste budget — and
  it is not a registered progress or learning invocation. Conversation
  transcript tools likewise have no foreground scope there.
- **Delivery** / **background continuation**: baseline + invocation-scoped tools
  (`mcp__right__send_progress`, `mcp__right__skill_learning_start`,
  `mcp__right__skill_learning_finish`),
  same rationale as cron. Conversation transcript tools likewise have no
  foreground scope there.
- **Learning prefilter** (`bot::learning_prefilter`): no MCP config and
  `--tools ""`; it is a classifier and never writes skill files.

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

## Idle compaction

Source of truth: `crates/bot/src/idle_compaction.rs`.

After a foreground chat session goes idle, the context window may be large but
cold — the prompt cache (≈5-minute TTL) has long expired, so the user's next
turn pays full-price input on the entire accumulated history. Idle compaction
drives CC's native `/compact` automatically to shrink the session before the
user returns.

**Debounce lifecycle.** A `CompactTimers` map (`Arc<DashMap<(i64, i64),
CancellationToken>>`, keyed by `(chat_id, thread_id)`) holds one timer per
active chat. The map is in-memory only — it is lost on bot restart and
re-armed on the next message. For **Normal-mode foreground turns only** (never
cron, delivery, reflection, or background), the worker cancels any existing
timer at turn start and arms a fresh one at turn end. Arming spawns a task
that `tokio::select!`s between a 2h sleep and the cancellation token: if the
token fires first (a new turn arrived), the task exits cleanly. If the sleep
wins first, the task keeps the same token in the map and runs an **abortable**
compaction. A turn that arrives during compaction cancels the token, which
drops the `wait_with_output_or_kill` future and SIGKILLs the `/compact`
process group (the in-sandbox grandchild too) — nothing is orphaned and the
session lock releases promptly. The map entry is left in place after the task
finishes; the next `arm`/`cancel` on that session cleans it up (a late
`cancel()` on a finished task is a harmless no-op).

**Gate.** Two conditions, both evaluated at arm time and re-checked at fire
time. First, the model must match `claude-opus*[1m]` (the `[1m]` suffix
selects 1M-context opus across version bumps, excluding sonnet[1m] and non-1M
opus). Second, the context footprint must be ≥ 400,000 tokens (40% of the 1M
window), read from the newest `interactive` row in `usage_events` for the
chat: `input_tokens + cache_read_tokens + cache_creation_tokens`. The model
check is evaluated first (in-memory `ArcSwap` load) so non-opus[1m] agents
never touch the DB on a turn.

**Self-limiting behavior.** Per-turn re-evaluation makes persistence
unnecessary. If CC auto-compacted mid-conversation, the next turn reports a
much smaller footprint and the gate fails, so no timer is armed. If our own
compaction ran, the session stays idle (no turn to re-arm) until the user
returns, regrows the context past 400k, and idles again. There is no
`compacted_at` marker and nothing to store.

**Fire-time re-check** covers the case where the agent was switched away from
opus[1m] via `/model` during the 2h idle window — the model is hot-reloadable,
so the arm-time model may not match the fire-time model. The fullness is also
re-queried at fire time for safety, though context cannot change without a turn
(which would have reset or cancelled the timer).

**Invocation.** When the gate passes, the fire task `try_lock`s the per-session
`SessionLocks` mutex (keyed by `root_session_id`, the same lock the worker
holds during every `--resume` turn) — **skipping** compaction entirely if the
session is busy, so it never starts on an active session and never queues
behind a live turn. It then runs a specialized maintenance invocation:
`claude -p --resume <root_session_id> "/compact <recency instruction>"` with no
`--mcp-config` and no `--json-schema` — `/compact` uses no tools and its
`result` field is empty. The wait races `wait_with_output_or_kill` (120s
`COMPACT_TIMEOUT`) against the session cancellation token; both timeout and
turn-activity abort drop the `ProcessGroupChild`, SIGKILLing the whole process
group, so the child is never orphaned. Success is `output.status.success()`.
Token usage is recorded via `insert_idle_compaction` (source `'idle_compaction'`).

**Edge cases.** A user returning mid-compaction is never stalled: a turn at
session-start cancels the timer (aborting the in-flight `/compact` and
releasing the lock at once), and `try_lock` means a turn already holding the
lock simply causes compaction to skip. Neither direction of lock contention
makes the user wait.

## Reflection Primitive

`crates/bot/src/reflection.rs` exposes `reflect_on_failure(ctx) -> Result<String, ReflectionError>`.
On CC invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in a tokened
`⟨⟨SYSTEM_NOTICE:<token>⟩⟩ … ⟨⟨/SYSTEM_NOTICE:<token>⟩⟩` (the per-agent token from
`right_mcp::credentials::get_or_create_notice_token`, also published in the
prompt's `## Platform Notice Token` section), so the agent produces a
human-friendly summary of the failure instead of the raw ring-buffer dump.

- Worker uses `ReflectionLimits::WORKER` (3 turns, $0.20, 90s process timeout).
  Reflection reply is sent to Telegram directly; on reflection failure, the
  caller falls back to the raw error message.
- Cron uses `ReflectionLimits::CRON` (5 turns, $0.40, 180s process timeout).
  Reflection reply is stored as a notify `async_runs.delivery_json`; async
  delivery picks it up and relays using `DELIVERY_INSTRUCTION_FAILURE`
  (non-verbatim — agent may rephrase lightly, must preserve facts).
- `usage_events` rows for reflection use `source = "reflection"`, discriminated
  by `chat_id` (worker parent) vs `job_name` (cron parent). `/usage` shows them
  on a separate "🧠 Reflection" line per window.
- Reflection never reflects on itself. Hindsight retain
  (`mcp__right__memory_retain` at the agent surface) is skipped for reflection
  turns.
- `async_runs.status` gates delivery: `'failed'` routes to
  `DELIVERY_INSTRUCTION_FAILURE`, any other status (currently `'success'`)
  routes to `DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).
- **Budget-exhausted cron failures skip reflection.** `--max-budget-usd` is a
  session-cumulative cap, and reflection `--resume`s the run's session — so
  reflecting a `BudgetExceeded` failure would immediately re-hit the cap (a
  futile, billable turn). `cron.rs` detects `FailureKind::BudgetExceeded` and
  reports a deterministic reason instead of calling `reflect_on_failure`.

### Structured-output loop guard

A `claude -p` turn can get stuck repeatedly emitting a `StructuredOutput`
tool_use whose payload fails schema validation, with CC re-prompting itself
after each rejection. Before the guard this loop was both invisible (the
rejection lines were never surfaced in the worker stream) and durable (it
survived restarts, because the resumed session's pending state was still an
unsatisfied structured-output demand), so the agent could burn turns and budget
without ever producing a deliverable.

The worker tracks consecutive rejections with a small `SchemaRejectionRun`
FSM while reading the stream:

- Each stream line is tested with `is_structured_output_rejection`; a match
  increments the run counter.
- The counter resets **only** on a successful `tool_result` — i.e. real
  forward progress. It is deliberately **not** reset by the intervening
  `StructuredOutput` tool_use that precedes a rejection, since that tool_use is
  the very thing being retried; resetting there would let an infinite
  use→reject→use→reject loop run forever.
- When the run reaches the threshold of **3** consecutive rejections, the
  worker kills the CC child and returns
  `InvokeCcFailure::Reflectable { kind: FailureKind::StructuredOutputLoop { rejections } }`,
  where `rejections` is the observed count.

Each detected rejection is now logged in the worker stream (the previously
invisible loop is observable). Routing the failure as `Reflectable` sends it
through the normal `reflect_on_failure` path, which `--resume`s the session for
a short summarizing turn and delivers a human-friendly failure message. This is
what breaks the restart-surviving loop: reflection replaces the pending
structured-output demand with a delivered summary, so the next resumed state is
no longer an unsatisfied schema request. Reflection runs on a separate path and
never feeds the detector, so the guard cannot recurse on its own reflection
turn.

### Null-reply repair

The delivery contract makes `content: null` mean "send nothing", which an
agent can emit by mistake after writing its reply as a plain assistant text
block (text blocks are never delivered). The worker detects the suspicious
shape and runs one repair resume instead of silently dropping the reply.

Trigger (all required, `worker_reply::null_reply_needs_repair`): `content` is
null; no attachments (media-only replies are legitimate nulls); no
`mcp__right__send_message` call this turn (terminal null after send_message is
sanctioned); the last assistant text block is non-empty. The worker captures
the evidence while streaming: `parse_persisted_stream_events` yields every
content block, so multi-block assistant messages cannot hide a `send_message`
call or a trailing text block.

The repair (`reflection::repair_null_reply`) shares the reflection plumbing
(`run_notice_resume`): `--resume` on the same session, `REPLY_SCHEMA_JSON`,
`ReflectionLimits::NULL_REPAIR` (= WORKER: 3 turns, $0.20, 90s), a tokened
SYSTEM_NOTICE that shows the discarded text and offers an explicit escape
hatch — "return `content: null` again to confirm intentional silence". Outcomes:

- repaired output has content or attachments → delivered through the normal
  reply path (receipts, threading, attachments all apply);
- second null → intentional silence confirmed, nothing sent, incident closed
  (this keeps group lurk mode safe);
- repair itself fails (spawn/timeout/parse) → the discarded text block is
  delivered raw as a last resort.

Repair runs at most once per turn and is never re-triggered by its own
output. Usage is accounted as `source = "reflection"` (worker parent); fires
are logged (`null reply with undelivered text block`) so prompt-side fixes
can be evaluated against the fire rate. Cron turns need no repair: the cron
schema makes silence an explicit `delivery.kind = "silent"` with a required
reason.

Cron success output stores `async_runs.run_note` plus a structured
`delivery_json` decision. `delivery.kind = "notify"` enters the async delivery
queue; `delivery.kind = "silent"` is a completed non-delivering run. The
delivery loop never uses `run_note` as fallback Telegram content.

Cron **failure** output derives its reason from the terminal CC `result` line
via `cron.rs::terminal_failure_detail`: CC error subtypes
(`error_max_budget_usd`, `error_max_turns`, `error_during_execution`) carry no
`result` text, so the detail is synthesized from `subtype` + `total_cost_usd`.
That detail feeds `classify_cron_failure`, the user-facing notice, and a
structured `async_runs.error_json` (`{kind, exit_code, failure, detail}`) that is
persisted regardless of whether reflection runs — so a failure reason is never
reduced to a bare exit code.

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

## Force-notify trigger

`mcp__right__cron_trigger(job_name, notify=true)` force-runs a job and
guarantees a prompt report. It sets `cron_specs.trigger_force_notify`
alongside `triggered_at` (both cleared together by `clear_triggered_at`).
The reconciler's triggered branch passes the flag via the in-memory
`CronSpec` to `execute_job`, which (1) prepends an "always notify, don't
go silent" tokened `⟨⟨SYSTEM_NOTICE:<token>⟩⟩` directive to the run prompt, and
(2) stamps `async_runs.force_notify = 1`. `persist_successful_cron_output` then forces
`delivery_required = 1` even on a silent decision (delivering the silent
reason as content), and the delivery loop's `should_hold_delivery` skips the
idle gate for force-notify rows — evaluated against the deduplicated delivery
candidate, so a forced newer run overrides even when an older non-forced run
is the oldest pending row. Force-trigger while the job is locked is dropped,
same as a plain trigger; the flag is transient, so scheduled runs of a
recurring job are unaffected.

`cron_trigger` can also carry a transient `then` and a resolved origin chat,
written to `cron_specs` next to `triggered_at` (same lifecycle, cleared together)
and loaded onto the in-memory `CronSpec`. On the triggered run's terminal
completion `execute_job` calls `resolve_then_action` (honoring `then.run_on` and
the target precedence `then.target_chat_id` > resolved origin > the job's
standing `target_chat_id`); if it fires, `spawn_then_continuation` forks the
triggered run's session via `spawn_background_continuation` — a normal
`kind='background'` row (`producer_ref='cron_then'`) so delivery and recovery
treat it like any continuation. Depth is capped at one: the continuation carries
no `then`. The failure-branch then-spawn runs after reflection.

Worker-created background rows start as `status = 'queued'` and
`handoff_state = 'queued'`. Startup recovery converts only those interrupted
queued handoffs into failed rows with pending delivery. It does not infer stale
`running` recovery without process ownership. Foreground background markers
read only `async_runs.kind = 'background'` rows for the chat, including running
rows and finished `success`/`failed` rows with `delivery_status IN
('pending', 'retryable')`.

When a spawned background continuation finishes but its terminal CC `result`
line carries `is_error: true` (e.g. a 529 overload that exhausted CC's internal
retries — which surfaces as `subtype: success`, `is_error: true`,
`api_error_status: 529` with no `structured_output`), `complete_background_run`
classifies it via `cron::classify_failed_result` and delivers a user-facing
explanation (transient-overload / rate-limit / HTTP-status / turn-limit) instead
of the generic `BACKGROUND_FAILURE_NOTIFY_CONTENT`. The raw status is kept only
in the internal `error_json`. This is deterministic — the background path never
runs reflection (reflection would re-issue the same overloaded call), unlike the
cron failure path. Infrastructure failures (reader/exit/parse) keep the generic
notice.

Process shutdown (`SIGINT`/`SIGTERM`) requests background handoff for active
foreground Telegram turns instead of dropping them. The worker uses the same
`async_runs kind='background'` continuation path as the Background button, but
with shutdown-specific logs and banner text. If handoff cannot be confirmed by
the continuation's `system/init`, the background row is marked failed with
pending delivery. Workers that have received Telegram messages but have not
started a foreground Claude invocation exit during shutdown instead of starting
new foreground work.

During shutdown, cron schedulers stop creating new runs. Running cron jobs get
the bounded `SHUTDOWN_JOB_TIMEOUT` drain. Jobs still running after that timeout
are aborted by the owning bot process and marked failed with a
`cron_shutdown_interrupted` error payload; targeted runs remain pending for
delivery. After the normal async delivery loop exits within its shutdown
deadline, the shutdown delivery flush sends already-terminal pending async
results without waiting for chat-idle politeness, then exits. The shutdown
deadline is applied before Telegram send side effects as a safe interruption.
Once a Telegram delivery request is in flight, a shutdown timeout is treated as
an unknown send outcome and marked terminal failed instead of retried, avoiding
duplicate partial delivery on restart.

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
