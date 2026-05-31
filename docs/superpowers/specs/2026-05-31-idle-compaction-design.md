# Idle `/compact` for opus[1m] sessions

- **Date:** 2026-05-31
- **Status:** Approved design — ready for implementation plan
- **Scope:** Bot worker + a new `idle_compaction` module; one new usage source; ARCHITECTURE invocation-contract exception.

## Problem

Long opus[1m] (1M-context) chat sessions accumulate hundreds of
thousands of tokens. When such a session goes quiet and the user returns
hours later, the Anthropic prompt cache (≈5-minute TTL) is long cold, so
the next turn pays **full-price input on the entire accumulated context**
— at the opus 1M pricing tier this is the single most expensive moment in
a session's life. Nothing currently shrinks a cold, bloated session
before the user comes back.

Claude Code already knows how to summarize a session in place via
`/compact`, and (verified, see Background) it works headlessly over
`--resume` and persists. We want to drive it automatically: after a chat
has been idle long enough that the cache is certainly cold, collapse the
context **once**, so the user's next turn re-reads a small summary instead
of the full history.

## Goals

- Automatically run CC's native `/compact` on a foreground chat session
  (DM, group, topic — any `(chat_id, thread_id)`) after **2h of
  inactivity**, but only when it is worth it.
- Gate strictly: only **opus with the 1M window**, and only when the
  session has consumed **≥40% of the 1M window (≥400,000 tokens)**.
- Steer the compaction summary toward the **most recently discussed
  topics** so continuity on return is good.
- Never race a live turn; never message the user; never disturb learning,
  memory, or delivery.
- Keep cost visible: record the compaction's token spend as a usage event.

## Non-goals

- No periodic/polling sweep. The trigger is event-driven (debounce).
- No persistence of pending compactions across bot restart (in-memory
  only; re-armed on the next message).
- No separate handoff document or post-compact injection turn. The
  recency steering is folded into the `/compact` instruction itself.
- No per-agent config surface. Thresholds are hardcoded constants;
  the feature is implicitly active whenever the agent runs opus[1m].
- No cheaper-summary-model optimization in v1 — compaction runs on the
  session's own model (opus[1m]). See *Future*.
- `sonnet[1m]` and non-1M models are out of scope (opus[1m] only).

## Background

### How Hermes does it (research)

`/Users/developer/dev/hermes-agent/agent/context_compressor.py` runs its own
agent loop over an OpenAI-style `messages[]` array and compacts that array
directly: prune old tool outputs → protect a head (`protect_first_n=3`)
→ protect a **token-budgeted tail** (≈20% of the threshold, with a
`protect_last_n=20` floor) → summarize the middle with a cheap auxiliary
model into a structured template, iteratively updated on re-compaction →
sanitize orphaned tool-call/result pairs. Its default trigger is a
**token threshold (50%) checked during the loop**, not time/idle based.
The tail-budget protection is its "preserve recent messages" mechanism.

**Why this does not port:** RightClaw does not own the message array —
Claude Code owns it inside the `--resume` session transcript. We cannot
manipulate it. So we delegate the collapse to CC's native `/compact`,
which already keeps a recency-aware summary the way Hermes does by hand.
Hermes' research only confirms what good preservation looks like and that
recency-weighting the summary is the right instinct.

### CC `/compact` capabilities (verified, CC v2.1.158)

- `/compact [instructions]` accepts free-text focus instructions.
- `claude -p --resume <id> "/compact <instructions>"` **works headlessly
  and persists** — the next `--resume` continues from the compacted state
  (verified by live test).
- `/compact` **replaces the conversation with a structured summary** (it
  does *not* keep recent messages verbatim). This is exactly why the
  recency instruction does real work: it steers what the summary keeps.
- The JSON `result` field comes back **empty** on a compact call. Success
  must be judged by process exit status, not result content.
- System prompt, project `CLAUDE.md`, and auto-memory are re-injected from
  disk after compaction; invoked skill bodies are re-injected (capped).

## Design

### Overview

A per-`(chat_id, thread_id)` in-memory debounce timer. Activity cancels a
pending compaction; 2h of true silence on a qualifying opus[1m] session
fires one silent `/compact`. The compaction is a specialized maintenance
`claude -p` invocation modeled on the existing learning prefilter, run
under the existing per-session `--resume` mutex.

### Components and files

**New module — `crates/bot/src/idle_compaction.rs`.** Owns:

- Constants (below).
- `CompactTimers` type + `arm` / `cancel` helpers over the timer map.
- `eligible(...)` gate predicate (model + fullness).
- `latest_interactive_context_tokens(conn, chat_id, thread_id)` — the
  fullness query.
- `run_compaction(ctx)` — the specialized invocation (mirrors
  `learning_prefilter::run`), including fire-time re-check and the
  session-lock acquisition.

Keeping the logic here avoids growing `worker.rs` (already ~5,600 lines);
`worker.rs` gets only two thin hook calls.

**Type alias — `crates/bot/src/telegram/mod.rs`** (next to `SessionLocks`,
line ~76):

```rust
/// Per-(chat_id, thread_id) idle-compaction debounce timers.
/// Aborting the handle cancels a queued compaction. In-memory only.
pub(crate) type CompactTimers =
    Arc<DashMap<(i64, i64), tokio::task::AbortHandle>>;
```

**Construction — `crates/bot/src/lib.rs`** next to `session_locks`
(line ~1044):

```rust
let compact_timers: crate::telegram::CompactTimers = Arc::new(DashMap::new());
```

Threaded into the worker the same way `session_locks` is.

**`WorkerContext` — `crates/bot/src/telegram/worker.rs`** (~line 280):
add `pub compact_timers: crate::telegram::CompactTimers`.

### Debounce lifecycle

Two hook points in `worker.rs`, both **only for `PromptMode::Normal`
foreground turns** (never cron / delivery / reflection / background):

1. **Turn start** — before `invoke_cc` runs for an incoming Normal
   message: `idle_compaction::cancel(&ctx.compact_timers, chat_id,
   eff_thread_id)`. Activity immediately aborts any queued compaction.

2. **Turn end** — an **independent** hook placed right after the existing
   post-turn learning block (~line 2110), **not nested inside it** (that
   block is gated on `learning.prefilter_enabled`; the debounce must run
   regardless). It evaluates the gate and:
   - **passes** → abort any existing handle for the key, then spawn the
     fire task (sleeps `IDLE_AFTER`, then compacts) and store its
     `AbortHandle`. This *resets* the debounce on every qualifying turn.
   - **fails** → `cancel(...)` the key (ensures no stale timer survives,
     e.g. right after CC auto-compacted the context below threshold).

The turn-end hook clones from `ctx` exactly what the learning block
already clones: `agent_dir`, `agent_db_dir`, `agent_name`,
`ssh_config_path`, `resolved_sandbox`, `Arc::clone(&ctx.model)`,
`ctx.session_locks.clone()`, `Arc::clone(&ctx.debug)`,
`ctx.compact_timers.clone()`, plus `chat_id` and `eff_thread_id`.

### Gate

All required, evaluated at arm time **and** re-checked at fire time:

- **Model:** `snapshot_model(&ctx.model)` is `Some(m)` with
  `m.starts_with("claude-opus") && m.ends_with("[1m]")`. Matching on the
  `[1m]` suffix rather than a hardcoded `claude-opus-4-8[1m]` keeps the
  feature working across opus version bumps while still excluding
  `sonnet[1m]` and non-1M opus. (`claude-opus-4-8[1m]` is the current
  id; see `telegram/model_command.rs:36`.)
- **Fullness:** `context_used ≥ MIN_USED_TOKENS` (400,000).

Evaluate the **model check first** and short-circuit: the fullness query
only runs when the model qualifies, so non-opus[1m] agents never touch the
DB on a turn.

The fire-time re-check matters because `model` is hot-reloadable (`/model`
can switch the agent away from opus[1m] during the 2h idle window) and is
one cheap DB read plus one `ArcSwap` load.

### Fullness signal

Direct and persisted — no live probe. At the turn-end hook the `interactive`
`usage_events` row for the just-finished turn is already written (it is
inserted inside `invoke_cc` at `worker.rs:3344`, which returns before the
post-turn block). Read the latest such row for `(chat_id, thread_id)`:

```sql
SELECT input_tokens + cache_read_tokens + cache_creation_tokens
FROM usage_events
WHERE chat_id = ?1 AND thread_id = ?2 AND source = 'interactive'
ORDER BY ts DESC
LIMIT 1
```

This sum is the prompt footprint going into the last API call — i.e. how
full the context is, regardless of how much was cache-served. The debounce
turn-end hook is its own (unconditional) spawn, separate from the
conditional learning spawn, so it opens its own short-lived connection for
this query; the fire task re-opens at fire time. To avoid a DB open on
every turn of non-opus[1m] agents, evaluate the **model check first**
(in-memory `ArcSwap` load) and only run the fullness query when the model
qualifies.

### Auto-compact interaction and idempotency

Both are handled structurally by per-turn re-evaluation, with **no special
detection and no `compacted_at` marker**:

- **CC auto-compacts mid-conversation** (near-full context): the next turn
  reports a much smaller context → the turn-end gate fails → the timer is
  cancelled. We never fire `/compact` on a freshly-auto-compacted (≈1%)
  session. This is the exact trap the user called out.
- **After our own compaction**, the context drops well below 40%. The
  session is still idle (no turn to re-arm it), so nothing re-fires until
  the user returns and regrows the context past 400k and then idles again.
  Self-limiting.

Because the value at arm time equals the value at fire time (context
cannot change without a turn, and a turn resets/cancels the timer), there
is no staleness between arming and firing.

### Compaction invocation (specialized maintenance contract)

`run_compaction` mirrors `learning_prefilter::run`
(`crates/bot/src/learning_prefilter.rs:477`). On fire:

1. Open the agent DB (`right_db::open_connection(&agent_db_dir, false)`).
2. **Re-check the gate** (model snapshot + fullness query). Bail quietly if
   it no longer holds.
3. Resolve the active session's `root_session_id` for
   `(chat_id, thread_id)` via the existing active-session lookup in
   `telegram::session`. Bail if there is no active session.
4. **Acquire the per-session mutex** — the same `SessionLocks` map keyed by
   `root_session_id` that the worker takes before every `--resume`
   (`telegram/mod.rs:76`, acquired at `worker.rs:2932`). This is the one
   real correctness coupling: it guarantees the compaction never runs
   concurrently with a live turn on the same session.
5. Build and run the invocation:

   ```rust
   ClaudeInvocation {
       mcp_config_path: None,        // /compact uses no tools
       json_schema: None,            // no structured output; result is empty
       output_format: OutputFormat::Json,
       model: None,                  // inherit the session's model (opus[1m])
       max_budget_usd: None,
       max_turns: None,              // single op; bounded by wall-clock timeout
       resume_session_id: Some(root_session_id),
       new_session_id: None,
       fork_session: false,
       allowed_tools: vec![],
       disallowed_tools: vec![],
       extra_args: crate::cc::invocation::disable_all_tools_args(),
       prompt: Some(format!("/compact {RECENCY_INSTRUCTION}")),
       debug_flag: Some(debug_flag),
   }
   ```

   Then `build_claude_command(&args, &agent_dir, ssh_config.as_deref(),
   resolved_sandbox.as_deref()).await`, stdin null, stdout/stderr piped,
   run under `tokio::time::timeout(COMPACT_TIMEOUT, cmd.output())`.

6. **Success = `output.status.success()`** (the `result` field is empty
   for compact — do not parse it). On non-zero exit / timeout / spawn
   error: log a `warn!` with redacted argv (reuse the prefilter's
   `redact_*` approach) and return. No retry (the next idle cycle will try
   again).
7. **Record usage:** `parse_usage_full(&stdout)` → a new
   `insert_idle_compaction(conn, &breakdown, chat_id, thread_id)` helper
   (source `'idle_compaction'`). On insert failure, `warn!` and continue —
   the compaction already happened and cannot be undone.
8. Remove the `(chat_id, thread_id)` entry from `CompactTimers`.

This is a deliberate exception to the standard session-bearing
`ClaudeInvocation` contract (no `--json-schema`, no `--mcp-config`), in the
same family as the learning callsites. ARCHITECTURE.md must record it (see
*Doc updates*).

### Recency instruction (fixed string)

CC has the full conversation in context at compaction time, so a static
instruction suffices — we do not compute "the latest topics" ourselves:

```
Prioritize the most recently discussed topics and any open or unresolved
threads. Preserve concrete details from recent exchanges — names, file
paths, decisions, values, and the user's current goal — over older,
settled context.
```

### Concurrency and edge cases

- **User returns mid-compaction:** turn-start cancellation aborts a timer
  that has not yet fired. If compaction has already *started* (holds the
  session lock), the incoming turn blocks on that lock until compaction
  finishes (≤ `COMPACT_TIMEOUT`). Given the 2h idle precondition, the odds
  of a return landing inside the tens-of-seconds compaction window are
  low; the bounded wait is accepted for v1. (See *Future* for abort-on-
  message.)
- **Active session changed between arm and fire:** impossible without a
  turn, and a turn cancels/resets the timer. Fire-time resolves the
  current active `root_session_id` anyway, so it always targets the right
  session.
- **Bot restart:** timers are in-memory and lost; the next message in any
  chat re-arms. Dropping a few pending compactions is harmless (this is an
  optimization, not correctness).

### Constants (`idle_compaction.rs`)

```rust
const IDLE_AFTER: Duration = Duration::from_secs(2 * 60 * 60); // 2h
const CONTEXT_WINDOW_TOKENS: u64 = 1_000_000;                  // opus[1m]
const MIN_USED_FRACTION: f64 = 0.40;
const MIN_USED_TOKENS: u64 = 400_000;                          // 40% of 1M
const COMPACT_TIMEOUT: Duration = Duration::from_secs(120);
const RECENCY_INSTRUCTION: &str = "Prioritize the most recently ...";
```

## Data flow

```
Normal foreground message for (chat, thread)
  └─ [turn start]  idle_compaction::cancel(timers, chat, thread)
  └─ invoke_cc → streams turn, writes interactive usage_events row, returns
  └─ post-turn learning block (unchanged, conditional)
  └─ [turn end, Normal only]  evaluate gate:
        read latest interactive context_used for (chat, thread)
        model is opus[1m]?  AND  context_used ≥ 400k?
          ├─ yes → abort old timer; spawn fire task (sleep 2h); store handle
          └─ no  → cancel(timers, chat, thread)

Fire task wakes after 2h of no new Normal turn:
  └─ open DB; re-check model + fullness                → bail if not eligible
  └─ resolve active root_session_id for (chat, thread) → bail if none
  └─ acquire SessionLocks[root_session_id]
  └─ claude -p --resume root "/compact <recency instruction>"  (silent)
  └─ status.success()? → insert_idle_compaction usage row
  └─ remove timers[(chat, thread)]
```

## Error handling

Per project FAIL FAST, with the documented exception for fire-and-forget
background tasks (the learning prefilter sets the precedent): the fire task
is spawned and detached, so it cannot propagate to a caller. Every failure
path (`open_connection`, fullness query, no active session, spawn, timeout,
non-zero exit, usage insert) logs a `warn!` (anyhow chains via `{e:#}`) and
returns. Compaction is best-effort; a failure just means the session is
retried on the next idle cycle. No error is silently swallowed — all are
logged — and none corrupts session state because the session lock serializes
against live turns.

## Testing strategy

TDD, narrowest-first, per `AGENTS.rust.md` cadence.

- **Pure unit tests (no sandbox), `idle_compaction.rs`:**
  - `eligible`: opus[1m] + ≥400k → true; opus[1m] + 399,999 → false;
    `sonnet[1m]` → false; non-`[1m]` opus → false; `None` model → false;
    a future `claude-opus-4-9[1m]` → true (suffix match).
  - `arm` then `arm` again replaces the handle (old aborted); `cancel`
    removes and aborts; `cancel` on absent key is a no-op.
  - The recency `/compact` prompt is assembled correctly
    (`prompt == "/compact " + RECENCY_INSTRUCTION`).
  - `latest_interactive_context_tokens`: in-memory DB seeded with rows of
    several sources returns the newest `interactive` sum; ignores other
    sources; `None`/0 when absent.
- **Invocation argv test:** `ClaudeInvocation` for compaction produces
  `--resume <id>`, the `/compact …` prompt after `--`, **no**
  `--json-schema`, **no** `--mcp-config`, tools disabled.
- **Usage insert:** `insert_idle_compaction` writes a row with source
  `'idle_compaction'`; assert it is **absent** from
  `right_agent::usage::LEARNING_SOURCES`.
- **No live `/compact` integration test in the default path.** If an
  end-to-end test is added it must be `#[ignore = "ci-claude: ..."]` with a
  `ci_claude_` prefix and acquire a real sandbox via `TestSandbox` per the
  live-sandbox rules — not part of routine `cargo test`.

**Cadence:** targeted `devenv shell -- cargo test -p bot idle_compaction`
(and `-p right-agent` for the usage helper) during the loop; **one final
`devenv shell -- cargo test --workspace`** before declaring done.

## Files to create / modify

| File | Change |
|---|---|
| `crates/bot/src/idle_compaction.rs` | **New.** Timer map helpers, gate, fullness query, `run_compaction`, constants, unit tests. |
| `crates/bot/src/lib.rs` | Construct `compact_timers`; thread into worker setup; `mod idle_compaction;`. |
| `crates/bot/src/telegram/mod.rs` | `CompactTimers` type alias. |
| `crates/bot/src/telegram/worker.rs` | `WorkerContext.compact_timers` field; turn-start `cancel` hook; unconditional turn-end gate/arm hook (Normal-mode only). |
| `crates/right-agent/src/usage/insert.rs` | `insert_idle_compaction` (mirrors `insert_learning_prefilter`, source `'idle_compaction'`). |
| `ARCHITECTURE.md` | Claude Invocation Contract: note idle-compaction as a specialized maintenance callsite (no schema / no MCP), like learning. |
| `docs/architecture/sessions.md` | Narrate the idle-compaction debounce + flow (descriptive home, cite-on-touch). |
| `PROMPT_SYSTEM.md` | No change expected (no system-prompt/tool changes); confirm during implementation. |

## Doc updates required

- **ARCHITECTURE.md → Claude Invocation Contract:** add idle-compaction to
  the set of specialized callsites that are explicit exceptions to the
  schema/MCP invariants (alongside the learning callsites). One sentence;
  stays under the 40k budget.
- **docs/architecture/sessions.md (cite-on-touch):** add a subsection
  describing the debounce lifecycle, the gate, the auto-compact
  interaction, and the session-lock coupling. This is the descriptive home
  for the walkthrough; ARCHITECTURE.md keeps only the rule.

## Alternatives considered

- **Hermes-style external summarizer over our own message array** —
  impossible; CC owns the transcript. Rejected.
- **Session rotation (retire the bloated session, open a fresh one seeded
  with a handoff doc)** — full control and no dependency on headless
  `/compact`, but loses CC's native summary and adds a handoff/seed
  subsystem. Rejected once headless `/compact` was verified to work and
  persist; native `/compact` is simpler and the user explicitly chose it.
- **Separate handoff doc generated pre-compact and injected post-compact**
  — made redundant by folding recency steering into the `/compact`
  instruction. Dropped to cut a whole subsystem.
- **Periodic sweep over sessions** — replaced by event-driven debounce: no
  scanning, naturally resets on activity, and re-evaluates fullness every
  turn (which is what neutralizes the auto-compact trap).
- **Cheaper summary model (`--model haiku` on the compact call)** — would
  cut the one-time compaction read cost without changing the session model
  (each real turn re-specifies `--model`). Deferred for summary fidelity;
  listed in *Future*.
- **Persist `compact_due_at` + re-arm at startup** — rejected; not worth
  the column and startup pass for a best-effort optimization.

## Risks and open questions

- **Headless `/compact` longevity:** verified on CC v2.1.158, but it is an
  undocumented-for-automation use of an interactive command. A CC upgrade
  could change behavior. Mitigation: success is gated on exit status and
  failures are non-fatal; a regression degrades to "no compaction," not
  breakage. Worth a smoke check on CC bumps.
- **`--resume` without `--mcp-config`:** expected fine (`/compact` uses no
  tools), but confirm during implementation that resuming a normally-MCP
  session without MCP config does not warn/err.
- **Compaction latency vs. session lock:** a return landing inside an
  in-flight compaction waits up to `COMPACT_TIMEOUT`. Accepted for v1.

## Future (out of scope)

- Cheap-summary-model variant (`--model haiku` for the compact read).
- Abort an in-flight compaction when a user message arrives (wire the
  per-`(chat,thread)` `stop_tokens` cancellation into `run_compaction`).
- Make thresholds / enable configurable if real usage warrants it.
