# Agent self-continuation primitive — design

**Date:** 2026-06-21
**Status:** Approved (brainstorm) — ready for implementation plan
**Topic:** Let an agent that started out-of-turn async work resume its own
session later to finish/collect it, instead of ending the turn and silently
halting.

## 1. Motivation

### The incident (agent "agent-b")

Agent `agent-b` (sandbox `openshell`, model `claude-opus-4-7[1m]`) ran a turn in a
Telegram group that:

1. launched **4 browser-use cloud sessions** via
   `mcp__right__browser-use__run_session` (Darty / Boulanger / Leroy Merlin /
   Castorama-batch — checking Midea PortaSplit pickup availability),
2. polled each once with `get_session` (still "running"),
3. emitted the final result text **"Waiting for browser-use sessions to
   progress."** and ended.

The turn's `result` event was `subtype: "success"`, `is_error: false`,
`duration_ms: 99153` (~99s, far under the 600s ceiling). No timeout, no error,
no backgrounding.

### Root cause

In Right, one Telegram message = one `claude -p` turn. A turn that returns its
final result is **over**; nothing re-invokes the agent. Backgrounding is
**orchestrator-only** — `BgReason::{AutoTimeout, UserRequested, Shutdown}`
(`crates/bot/src/telegram/worker.rs:479`, banners `:765-769`,
`CC_TIMEOUT_SECS = 600` `:51`). There is **no agent-initiated "resume my
session later"** primitive.

`browser-use` is a real external MCP server
(`https://api.browser-use.com/v3/mcp`, auth `header`) proxied through the
aggregator (`{server}__ → ProxyBackend`). `run_session` POSTs a task to the
browser-use cloud and returns a session handle; the work runs cloud-side,
decoupled from the turn. When the turn ended, the 4 cloud sessions kept running
but **nothing in Right was polling them**. No "backgrounded" notice was sent
because nothing was backgrounded — the turn simply succeeded. To the user it
"just stopped."

The agent assumed an implicit continuation that does not exist. We give it an
explicit one.

## 2. Goals / non-goals

### Goals

- A **general** agent-initiated self-continuation primitive (browser-use is the
  first consumer; equally usable for long Composio ops, remote builds,
  self-reminders, any "I started something that finishes out of turn").
- The continuation **resumes the foreground session** so it has full
  conversation context.
- **Idempotency by agent judgment**: when it fires, the agent decides
  report / re-arm / stay-silent based on what it sees.
- **Cancellation** by stable task id when the wait becomes moot.
- **Bounded**: hard ceilings prevent runaway; the give-up path is never silent.
- Reuse the existing `async_runs` task table, background-fork execution, and
  delivery path. No new control plane.

### Non-goals (YAGNI for now)

- Operator/Telegram user-facing cancel button or dashboard task view (future).
- Event/webhook wake — browser-use has no callback into Right; poll-based only.
- Making cron resume foreground sessions (cron stays isolated by design).

## 3. Decisions (from brainstorm)

| # | Decision |
|---|----------|
| Scope | General "resume me later" capability, not a browser-use-specific poller. |
| Idempotency | **Agent-judgment-primary**: continuation resumes full session context and the agent decides report/re-arm/silent. Orchestrator only does cheap **duplicate-collapse** (latest wins, others `superseded`). No cancel-on-unrelated-foreground-activity. |
| Bounding | **Agent-declared intent within hard system ceilings**: agent expresses check-in cadence + give-up deadline; platform hard-caps hops, wall-clock, and cumulative budget. Final/give-up hop **must** emit a user-facing message. |
| Mechanism | **Approach A** — agent-triggered, *scheduled* `kind='continuation'` row on `async_runs`, fired by the existing reconciler, executed by the existing background-fork path, reported via the existing delivery path. |
| Storage | **Reuse `async_runs`** as the unified task table (same as `cron`/`background`), stable `id` = continuation/task id returned to the agent. |
| Cancellation | Agent-initiated `continue_cancel(continuation_id)`; scope-verified server-side. |

### Rejected alternatives

- **Pure `cron_create` one-shot + prompt only.** Cron runs a *fresh isolated
  session* (`resume_session_id: None`) — the agent couldn't see the
  conversation and couldn't satisfy the idempotency judgment; it'd re-poll blind
  and risk double-reporting.
- **Deferred foreground self-turn (Approach C).** Cleaner UX in theory (single
  invocation, full foreground tools), but introduces a new "foreground turn with
  no live user" execution mode with its own concurrency/UX edges. Approach A
  reuses the already-tested background+delivery path.

## 4. Data model — reuse `async_runs`

New `kind='continuation'`. Reuses: `id` (PK = **stable continuation/task id**
handed to the agent), `source_session_id` (foreground session, re-forked each
hop), `run_session_id` (per-hop fork), `status`, `delivery_required`,
`delivery_status`, `delivery_attempts`, `delivery_json`, `run_note`,
`target_chat_id`, `target_thread_id`, `exit_code`, `log_path`, timestamps.

New columns (idempotent migration, `pragma_table_info` guard, defaults that keep
existing rows valid):

- `scheduled_at` TEXT — next due time (UTC). Reconciler fires on
  `scheduled_at <= now`.
- `deadline_at` TEXT — hard give-up time.
- `hop_count` INTEGER NOT NULL DEFAULT 0 — re-arm counter.
- continuation instruction — what to do on resume. Stored on the row (a
  dedicated column or a small JSON blob; implementation choice during planning).

Status lifecycle: `scheduled → running → success | failed`, plus terminal
`cancelled` and `superseded`.

**Invariant:** one row = one logical wait = one stable id. Re-arm updates the
*same* row (`scheduled_at`, `hop_count++`), so the id the agent holds stays
valid for the whole wait.

## 5. Tool surface (MCP server `right`)

- `mcp__right__continue_later` — args: `instruction` (what to do on resume),
  `check_in` (interval seconds, min-clamped), `give_up_after` (deadline;
  clamped to hard ceiling), optional `max_budget_usd`. Returns `continuation_id`
  + resolved schedule. Called from a foreground turn → creates a row. Called
  from inside a continuation hop with its own id → **re-arms** the same row.
- `mcp__right__continue_cancel` — args: `continuation_id`. Marks `cancelled`;
  no-op if already terminal.
- `mcp__right__continue_list` — pending continuations for the **current scope
  only**; lets the agent recover ids. Returns id, instruction summary, next
  check-in, deadline, hop_count.

**Scope is server-resolved, never agent-supplied** (same invariant as
`get_messages_by_id` / `thread_search`): the agent passes only a
`continuation_id`; the server verifies it belongs to the current
`(chat_id, effective_thread_id, session)` and treats out-of-scope ids as
absent/denied.

**Disallow rules:** `continue_*` are *not* foreground-only "send to user" tools
(unlike `send_message`) — allowed in foreground **and** continuation hops;
disallowed in cron / reflection / delivery invocations.

Update `with_instructions()` in both `memory_server.rs` and `aggregator.rs` and
all agent-facing tool-name references (skills, templates, codegen,
`PROMPT_SYSTEM.md`).

## 6. Execution flow

1. **Arm.** Foreground turn starts async work, then before ending calls
   `continue_later(...)` → row `scheduled`, `scheduled_at = now + check_in`,
   `deadline_at = now + give_up_after`. Returns id. (Prompt forbids ending a
   turn "waiting" without arming one.)
2. **Fire.** Reconciler (existing ≤5s tick) finds due rows
   (`status='scheduled' AND scheduled_at <= now`), acquires the per-session
   lock, and spawns a background continuation **forking the latest
   `source_session_id`** (reuse `spawn_background_continuation`). Resume prompt =
   stored instruction + the `continuation_id` + bounding state + the standard
   *"check whether this was already handled in the conversation; if so, stay
   silent"* idempotency directive. `status → running`.
3. **Resumed hop decides** (sees full conversation, incl. in-between messages;
   can call `get_session` etc.):
   - results ready, not yet reported → `notify.content` → `success` →
     delivery relays (resumes main session).
   - still running, within ceilings → re-arm (`continue_later` same id) →
     `notify.silent`, row back to `scheduled` (`scheduled_at = now + check_in`,
     `hop_count++`).
   - already handled / moot → `notify.silent`, no re-arm → `success` silent
     (self-finalizes; no delivery).
   - **ceiling hit** (deadline/hops/budget) → **forced** user-facing report
     ("browser-use still not done after X — here's what I have / stopping").
     Never silent on give-up.
4. **Cancel.** Any in-between foreground turn where the agent judges the wait
   moot (task finished out of band, user redirected/cancelled) → `continue_cancel(id)`
   → `cancelled`, never fires.
5. **Collapse.** If multiple `scheduled` continuations exist for the same
   logical wait, keep the latest and mark the rest `superseded` (reuse the cron
   delivery dedup pattern). The normal re-arm path updates in place and avoids
   this.

## 7. Concurrency

Reuse `SessionLocks` (per `source_session_id`) and `BgHandoffGates`. A
continuation hop and a live user turn serialize on the session mutex; the fork
reads source state read-only (multiple forks are safe) and always forks the
**latest** state, so it sees everything since arming. If a user turn holds the
lock, the reconciler retries on the next tick. `lock_ttl`/heartbeat reuse so a
crashed hop doesn't wedge the row.

## 8. Bounding (default ceilings — tunable)

- `check_in` min-clamp **~30s** (avoid hammering external APIs); default
  ~60–90s.
- `give_up_after`: agent-declared; **hard ceiling ~6h, per-agent configurable**
  (accommodates 30 min / 1 hr+ workloads); default ~30 min if unspecified.
- `max_hops` backstop ~120 (generous; ≥1h even at the 30s floor).
- **Cumulative `max_budget_usd` across the chain — the meaningful runaway
  guard** (per-hop polling is cheap); per-agent configurable.
- Whichever ceiling fires first ends the chain; the terminal hop is **forced to
  notify**.

## 9. Prompt fix (the other half)

`OPERATING_INSTRUCTIONS.md` (prompt-tier brevity — a few declarative sentences):
a turn ends when you stop and you are not auto-resumed; if you started work that
finishes out of band, arm a continuation with `mcp__right__continue_later` and
never end a turn merely "waiting"; when resumed, first check whether it was
already handled and stay silent if so; if the user redirects or the work becomes
moot, cancel it with `mcp__right__continue_cancel`. Keep
`PROMPT_SYSTEM.md` in sync.

## 10. Cross-cutting (project conventions)

- **Codegen category:** prompt/MCP schema are `Regenerated(BotRestart)` —
  existing agents adopt via `right restart <agent>`, no sandbox recreation,
  backward-compatible defaults. (No new on-disk per-agent file; this is schema +
  DB.)
- **Migration:** additive `async_runs` columns, idempotent
  (`pragma_table_info`), registered in `right_db::migrations::MIGRATIONS`.
- **Transaction rule:** multi-write operations (arm/re-arm/collapse/cancel +
  status transitions) use a single immediate transaction.
- **Brand/UX:** any new user-facing text via `right_ui::*`; Telegram HTML
  escaped.

## 11. Reuse map (existing components to build on)

| Concern | Existing component | Ref |
|---|---|---|
| Task table | `async_runs` (kind cron/background) | `right-db` v25 schema; `right-agent/src/async_runs.rs` |
| Background fork of foreground session | `spawn_background_continuation` | `crates/bot/src/background.rs:40-192` |
| Continuation prompt builder | `build_continuation_prompt` | `crates/bot/src/telegram/worker.rs:734-761` |
| Scheduled firing tick | cron reconciler (≤5s) | `crates/bot/src/cron.rs` (`reconcile_jobs`) |
| Delivery (resumes main session, retry, dedup) | `async_delivery` | `crates/bot/src/async_delivery.rs` |
| Duplicate-collapse pattern | `deduplicate_job` | `crates/bot/src/async_delivery.rs:122-179` |
| Per-session serialization | `SessionLocks`, `BgHandoffGates` | `crates/bot/src/telegram/mod.rs:91,116-143` |
| Agent MCP tool plumbing | cron tools | `crates/right/src/memory_server.rs` cron_* |
| Foreground-only disallow helper | `disallow_foreground_only_tools` | background/cron callsites |

## 12. Testing

TDD per behavior; targeted commands during dev, full workspace suite at the end.

- Migration: additive columns idempotent; re-run safe.
- `async_runs` continuation lifecycle: arm → scheduled; reconciler fires due
  rows only; re-arm updates same row (`hop_count++`, new `scheduled_at`);
  collapse marks older `superseded`.
- Cancel: scope-safe (out-of-scope id absent/denied); no-op on terminal.
- Bounding: deadline/hops/budget each end the chain; terminal hop forces a
  delivery (never silent).
- Idempotency: a hop that sees the work already reported finalizes `silent`
  (no delivery).
- Concurrency: continuation hop and foreground turn serialize on the session
  lock; fork sees in-between messages.
- Tool scope enforcement: `continue_list` returns only current-scope rows.
- Final: `cargo nextest run --workspace` + `cargo test --doc --workspace`.

## 13. Open items to confirm during planning

- Exact default ceiling numbers (§8).
- Final tool names (`continue_later` / `continue_cancel` / `continue_list` are
  working names).
- Where the instruction blob lives on the row (dedicated column vs JSON).
