# Background Continuation for Long-Running Turns

**Date:** 2026-05-05
**Status:** Design (pending implementation plan)

## Problem

The bot worker currently kills any `claude -p` foreground turn that exceeds 600 seconds (`CC_TIMEOUT_SECS` in `crates/bot/src/telegram/worker.rs:44`). On kill, `reflect_on_failure` produces a short summary of partial progress and replies to the user. The user-visible result is "agent timed out" with a sketch of what it tried — useful diagnostically, useless as an answer.

There is no manual mechanism for a user to offload a still-running turn to the background while continuing to chat.

## Goal

When a foreground turn cannot finish in time, or when the user opts in mid-flight, fork the conversation into a background CC session that completes the work autonomously and delivers the final answer back into the main conversation. The user is unblocked immediately, can keep messaging the agent, and receives the deferred result whenever it lands.

Background work runs on the existing cron infrastructure with one new schedule variant. Multiple background jobs may run in parallel.

## Non-goals

- Cancelling a running background job from the UI. (Future feature: MCP cancel tool + maintenance commands.)
- Showing background-job progress to the user (ring buffer / cost / activity). Future feature.
- Letting the agent self-trigger a background job via MCP. The `Immediate` schedule becomes available to `cron_create` only as a side-effect of this work; explicit MCP exposure is a separate change.
- Reworking the existing cron failure reflection pipeline. Background failures use it unchanged.

## Decisions summary

| Decision | Choice |
|---|---|
| Trigger | Auto on 600s safety timeout AND manual `🌙 Background` button click. Other failure kinds (BudgetExceeded, MaxTurns, NonZeroExit) keep going to reflection. |
| Session model | Real CC `--fork-session` from the main session. No re-feeding of original prompt; forked session inherits full history. |
| Mechanism | Cron infrastructure extended with `ScheduleKind::Immediate`. Same execution path as existing `OneShot { run_at }` jobs. |
| Visibility | Background jobs appear in `/cron list` as ordinary one-shots (`bg-{HHMMSS}-{rand4}`). No filtering. |
| Concurrency | Multiple parallel background jobs allowed. Per-session mutex serialises only the moments when worker or delivery `--resume`s the same main session. |
| Limits | Default budget raised to $5 (applies to all new crons). No process timeout — already absent for cron jobs and remains so. Foreground retains its 600s limit. |
| Banner during background | Static text, no buttons. Cancel/progress controls deferred. |
| Race protection | Per-session `Arc<Mutex<()>>` keyed by main `root_session_id`, held by worker and delivery during their `--resume` calls. `IDLE_THRESHOLD_SECS = 180` retained as UX politeness only. |

## Data flow

### Foreground turn (worker.rs)

```
User message → debounce → invoke_cc → claude -p
Thinking message in chat: [⛔ Stop] [🌙 Background]

Three exits from invoke_cc:
  1. Normal finish      → reply, delete thinking message.       (unchanged)
  2. ⛔ Stop button     → kill child, "Stopped" banner.          (unchanged)
  3. 🌙 Background or
     600s auto-timeout  → kill child
                        → enqueue_background_job (cron_specs INSERT, schedule_kind=Immediate)
                        → edit thinking message to per-reason banner
                        → return Ok(None) — debounce frees, user can send next message
```

### Background execution (cron.rs)

```
Cron tick (5s) finds ScheduleKind::Immediate row → fire_job
  → claude -p --resume <main_session_id> --fork-session --session-id <bg_run_id>
  → CC copies main session history into <bg_run_id>.jsonl, appends continuation prompt as new user turn
  → Agent works to completion within budget, produces structured `notify` output
  → cron_runs row updated with status (success | failed) and notify_json
  → cron_specs row auto-deleted (one-shot, like OneShot variant)
```

### Delivery (cron_delivery.rs)

```
Polling loop (30s) → fetch_pending → ready row
  → acquire per-session Mutex for main_session_id (waits if worker holds it)
  → IDLE_THRESHOLD_SECS = 180 still applies as UX politeness
  → deliver_through_session: claude -p --resume <main> via Haiku
       stdin: YAML "[Result of your background work bg-XXX: ...]"
  → Main session gets a synthetic user-turn + agent's relayed reply
  → Reply sent to Telegram chat
  → cron_runs marked delivered
```

### Per-session mutex

```
SessionLocks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>
              key = main root_session_id

Acquired by:
  - worker::invoke_cc          before claude -p --resume <main>
  - cron_delivery::deliver_through_session   before claude -p --resume <main>

NOT acquired by:
  - cron::fire_job             uses --fork-session → writes to a NEW jsonl, no race
  - reflection::reflect_on_failure   resumes the killed bg session, not main

Held duration:
  - Foreground: up to 600s (whole CC turn).
  - Delivery:   ~5–15s (Haiku relay).

Cleanup: hourly soft sweep, drop entries where Arc::strong_count == 1.
```

## Components and code changes

### `right-agent/src/cron_spec.rs` — `ScheduleKind::Immediate`

Add new variant:
```rust
pub enum ScheduleKind {
    Cron(String),
    OneShot { run_at: DateTime<Utc> },
    Immediate,
}
```

DB encoding (no schema migration): sentinel string `'@immediate'` in existing `schedule TEXT` column, `run_at = NULL`, `recurring = 0`. Deserializer special-cases the sentinel before invoking `cron::Schedule::from_str`.

`resolve_schedule_fields` extended to a 3-way mutual exclusion across `schedule | run_at | immediate`. New parameter `immediate: bool` joins the existing two.

`is_one_shot()` returns `true` for both `OneShot` and `Immediate` (both auto-delete after firing).

`cron_schedule()` returns `None` for `Immediate` (no human-readable schedule string; `/cron list` displays `<immediate>` like it currently displays `<run_at>` for OneShot).

### `bot/src/cron.rs` — fire Immediate jobs

In the same reconcile pass that fires overdue `OneShot { run_at }` (around line 936–944), add a parallel filter for `ScheduleKind::Immediate` that fires unconditionally on every tick. Lock acquisition and `cron_runs` accounting are identical to the OneShot path.

### `bot/src/telegram/invocation.rs` — `--fork-session` support

`ClaudeInvocation` gets a new field:
```rust
pub fork_session: bool,
```

When `fork_session = true` AND `resume_session_id.is_some()`, `into_args()` emits `--fork-session` flag alongside `--resume <id>`. New session id (if also set via `new_session_id`) is the forked id.

Background job uses:
```rust
ClaudeInvocation {
    resume_session_id: Some(main_session_id),
    new_session_id: Some(bg_run_id),
    fork_session: true,
    prompt: Some(continuation_prompt),
    max_budget_usd: Some(5.0),
    ..
}
```

### `bot/src/telegram/worker.rs` — trigger handling

#### Keyboard (line 49–57)

```rust
fn working_keyboard(chat_id: i64, eff_thread_id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("⛔ Stop",       format!("stop:{chat_id}:{eff_thread_id}")),
        InlineKeyboardButton::callback("🌙 Background", format!("bg:{chat_id}:{eff_thread_id}")),
    ]])
}
```

Existing `stop_keyboard` callsites are updated to use `working_keyboard`.

#### State for "background requested"

New context field in `WorkerCtx`:
```rust
pub bg_requests: Arc<DashMap<(i64, i64), ()>>,
```

Set by the bg callback handler before cancelling the worker's stop token.

#### `invoke_cc` outcome

`InvokeCcFailure` gains a variant:
```rust
Backgrounded {
    reason: BgReason,
    main_session_id: String,
    thinking_msg_id: Option<MessageId>,
}

enum BgReason {
    AutoTimeout,
    UserRequested,
}
```

Replaces the `SafetyTimeout` reflection path (`SafetyTimeout` is no longer emitted by worker). After `child.kill()` and `child.wait()`:
- If `bg_requests` flag set → `Backgrounded { reason: UserRequested, .. }`
- Else if `timed_out` → `Backgrounded { reason: AutoTimeout, .. }`
- Else (other failures) → existing branches unchanged.

#### `enqueue_background_job`

New helper at the end of `worker.rs`:
```rust
fn enqueue_background_job(
    conn: &Connection,
    chat_id: i64,
    thread_id: i64,
    main_session_id: &str,
    reason: BgReason,
) -> Result<String, BotError> {
    let job_name = format!("bg-{}-{}", Utc::now().format("%H%M%S"), random_4chars());
    let prompt = build_continuation_prompt(reason);
    insert_immediate_cron(conn, &job_name, &prompt, chat_id, thread_id, /*budget=*/ None)?;
    Ok(job_name)
}
```

`insert_immediate_cron` lives in `cron_spec.rs` next to `insert_one_shot`; takes `target_chat_id`, `target_thread_id`, optional `max_budget_usd` (None ⇒ default).

#### `build_continuation_prompt`

```rust
fn build_continuation_prompt(reason: BgReason) -> String {
    let reason_text = match reason {
        BgReason::AutoTimeout => "the foreground turn hit the 10-minute safety limit and was terminated",
        BgReason::UserRequested => "the user moved this work to background execution",
    };
    format!("\u{27e8}\u{27e8}SYSTEM_NOTICE\u{27e9}\u{27e9}\n\
You were forked from the main conversation because {reason_text}.\n\
The previous turn did not complete. Please continue and produce a final\n\
answer to the user's MOST RECENT MESSAGE.\n\
\n\
Earlier conversation history is provided as context only — do not re-engage\n\
with it unless directly required to answer the most recent message.\n\
\n\
Take as much time as you need within your budget. Your reply will be relayed\n\
back to the main conversation, so write it as if responding to the user\n\
directly.\n\
\u{27e8}\u{27e8}/SYSTEM_NOTICE\u{27e9}\u{27e9}")
}
```

#### Banner text

After backgrounding, edit thinking message to:
- `BgReason::AutoTimeout`: `⏱ Foreground hit 10-min limit — continuing in background. Will reply when ready 🌙`
- `BgReason::UserRequested`: `🌙 Working in background. Will reply when ready`

Reply markup is cleared on both.

### `bot/src/telegram/handler.rs` — bg callback handler

Mirror of `handle_stop_callback`. Parses `bg:{chat_id}:{thread_id}`, looks up the cancellation token in `StopTokens`, sets the `bg_requests` flag, cancels the token, replies to the callback with "Sending to background…".

Dispatch in `dispatch.rs` adds a branch for `data.starts_with("bg:")`.

### `bot/src/telegram/mod.rs` and `lib.rs` — wiring

```rust
pub(crate) type SessionLocks = Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>;
pub(crate) type BgRequests = Arc<DashMap<(i64, i64), ()>>;
```

Both initialised once in `lib.rs::run_bot` and threaded into `WorkerCtx`, `handler.rs`, and the delivery loop config struct.

### `bot/src/cron_delivery.rs` — mutex acquisition + downgrade idle gate

`deliver_through_session` takes a `SessionLocks` clone; before spawning the Haiku CC call, awaits `session_locks.entry(main_session_id).or_insert_with(default).lock().await`.

`IDLE_THRESHOLD_SECS` retained but documented as a UX politeness gate, not a correctness mechanism.

### `cron_spec.rs` — default budget

```rust
- pub const DEFAULT_CRON_BUDGET_USD: f64 = 2.0;
+ pub const DEFAULT_CRON_BUDGET_USD: f64 = 5.0;
```

Applies to new cron rows created without an explicit `--budget`. Existing rows keep their stored value.

### `worker.rs` — mutex acquisition in invoke_cc

Before `claude -p --resume <main>`, acquire the per-session mutex. Drop on exit (`_guard` dropped naturally after `child.wait()` returns and outcome is constructed).

For the very first turn of a session (`is_first_call = true`, no main session yet), no lock needed — `--session-id <new>` cannot race with anything.

## Risks and trade-offs

### Partial last turn in main session

When foreground is killed mid-turn, `<main>.jsonl` may contain incomplete trailing entries (e.g. `assistant-message-start` without close, partial `tool_use`). `--fork-session` copies the file verbatim, so the forked session inherits any garbage.

CC's behaviour on partial trailing entries is not publicly documented. Mitigation strategies if observed:
- Truncate `<main>.jsonl` to the last valid `user` entry before fork.
- Append a synthetic `assistant` message marking interruption.

Treat as TO VERIFY during implementation. If empirically robust, no action needed.

### Cost on long-history channels

Forked session inherits the full chat history. For long-running channels, the first forked turn carries a large context. Prompt caching (Anthropic side) should hit on identical prefixes, but the first uncached read can be expensive. Bounded by the $5 default budget; acceptable.

### Hung background jobs without time limit

CC checks `--max-budget-usd` only between turns. A turn that hangs inside a tool call or a sandbox network deadlock will never complete a turn, never trigger the budget check, and never exit. The lock file's heartbeat (`crons/.locks/<job>.json`) keeps refreshing until the bot process restarts.

Detection today: `/cron list` plus per-job log inspection. Recovery: bot restart kills the orphan process. Future: MCP cancel tool. Not a blocker for shipping.

### Delivery failure dropping bg results

After 3 delivery attempts, `cron_delivery` marks the run as `'failed'` and stops retrying. The user receives nothing. This is existing cron behaviour; we adopt it. Known limitation, monitor in production.

### Idle-gate edge case: never-idle chats

Channels where the user types every minute may keep `IDLE_THRESHOLD_SECS = 180` from ever satisfying. Background results queue indefinitely. Same behaviour as existing crons today; not new.

### Multiple parallel backgrounds for one chat

Three concurrent background jobs from the same chat all end up wanting to deliver into the same main session. They serialise naturally:
- `fetch_pending` returns one row per delivery tick.
- Per-session mutex blocks delivery while worker holds it (or vice versa).

Worst case: a chat with three pending deliveries and an active foreground sees deliveries land minutes apart. Acceptable.

## Failure mode (background CC fails)

Reuses existing cron failure pipeline:

1. `classify_cron_failure` (`cron.rs:620`) categorises the failure.
2. `reflect_on_failure` (`reflection.rs`) runs a 5-turn / $0.40 / 180s reflection on the failed bg session.
3. Reflected text stored in `cron_runs.notify_json` with `status='failed'`.
4. `cron_delivery` picks up the row, uses `DELIVERY_INSTRUCTION_FAILURE` template.
5. Main session receives synthetic message "[your background job failed: <summary>]"; agent rephrases and sends to Telegram.

Reflection self-recursion is already prevented by existing `reflection.rs` checks. No new infinite-loop risk.

## Testing strategy

### Pure unit tests

| Test | File | Verifies |
|---|---|---|
| `immediate_sentinel_roundtrip` | `cron_spec_tests.rs` | `Immediate ↔ '@immediate'` round-trip |
| `resolve_schedule_fields_immediate_mutex` | `cron_spec_tests.rs` | 3-way mutual exclusion across schedule/run_at/immediate |
| `validate_immediate_implies_one_shot` | `cron_spec_tests.rs` | `Immediate` always `recurring=0` |
| `build_continuation_prompt_auto_timeout` | `worker.rs::tests` | reason text + focus-on-latest hint present |
| `build_continuation_prompt_user_requested` | `worker.rs::tests` | reason text |
| `working_keyboard_two_buttons` | `worker.rs::tests` | 2 buttons with correct callback_data |
| `parse_bg_callback_data_valid` | `handler.rs::tests` | `"bg:42:7"` parses |
| `parse_bg_callback_data_malformed` | `handler.rs::tests` | error cases |
| `enqueue_background_job_inserts_immediate_row` | `worker.rs::tests` | INSERT to in-memory SQLite produces correct row |

### Integration tests (TestSandbox)

| Test | Verifies |
|---|---|
| `immediate_job_fires_on_next_tick` | Insert Immediate row → `cron_runs.status='success'` within 6s |
| `bg_job_uses_fork_session` | Captured `claude -p` argv contains `--resume <main>`, `--fork-session`, `--session-id <bg_run_id>` |
| `session_mutex_serializes_resume` | Two parallel `--resume <main>` actors are serialised |
| `auto_timeout_path_no_reflection` | Lower CC_TIMEOUT_SECS via `cfg(test)`; verify Immediate row created and `usage_events` has no reflection rows for that turn |
| `bg_button_path_creates_immediate_job` | Simulated bg callback → kill → cron_specs row inserted with correct schedule sentinel |
| `bg_failure_delivers_as_failure_message` | Bg with failing prompt → reflection → delivery uses `DELIVERY_INSTRUCTION_FAILURE` |

### TDD ordering

1. `cron_spec`: Immediate model + DB serialisation (pure).
2. Cron loop fires Immediate (integration).
3. `ClaudeInvocation` accepts `fork_session`; `build_continuation_prompt` (pure).
4. `enqueue_background_job` helper (pure → DB).
5. Per-session Mutex wiring (integration with two resumers).
6. UI keyboard + bg callback parser (pure).
7. Worker timeout/button → enqueue path (integration with lowered timeout).

## Migration & rollout

### Required steps

1. Code lands; bot processes restart via `process-compose on_failure` or `right restart <agent>`. New code is `Regenerated(BotRestart)` category — picked up automatically.
2. ARCHITECTURE.md updates:
   - **Per-message flow**: add `🌙 Background` branch.
   - **Cron Lifecycle**: add `ScheduleKind::Immediate`.
   - **Sandbox session race protection**: per-session Mutex; idle gate as UX politeness.

### Not required

- DB schema migration (sentinel approach, no new column).
- Sandbox recreate (no filesystem-policy change).
- Manual operator action for already-deployed agents.

### Compatibility with existing crons

- New default `DEFAULT_CRON_BUDGET_USD = 5.0` applies only to crons created without explicit `--budget`. Stored rows keep their explicit values.
- New `ScheduleKind::Immediate` variant requires updates to all exhaustive matches on `ScheduleKind`. Compiler-enforced; locate via `rg "ScheduleKind::"`. Known callsites:
  - `cron_spec.rs::cron_schedule()`, `is_one_shot()`
  - `cron.rs:1036` schedule display
  - `cron.rs:936` overdue reconcile
  - `cron_spec.rs::resolve_schedule_fields`

### Rollback

Revert the commit. Existing crons keep working. If a user clicked `🌙 Background` between deploy and rollback, the corresponding `cron_specs` row is now an Immediate sentinel that the old code does not recognise; it sits dormant. Cleanup: `DELETE FROM cron_specs WHERE schedule='@immediate'` per agent DB.

## Future work (out of scope)

- MCP `cron_create` exposes `--immediate` flag for agent self-scheduling.
- MCP cancel tool: agent or user can cancel a running cron job (kills CC, deletes lock and row).
- Cancel-during-background button on the banner (depends on cancel tool).
- Show-progress button on the banner (live ring buffer + cost from background NDJSON log).
- Per-agent override for `DEFAULT_CRON_BUDGET_USD` via `agent.yaml`.
- Lower `IDLE_THRESHOLD_SECS` to 30s or remove once the per-session mutex has run in production for a release cycle.
