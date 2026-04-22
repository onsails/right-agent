# Error Reflection Design

**Date:** 2026-04-21
**Status:** Draft

## Problem

When an agent invocation fails — safety timeout (600s), non-zero exit, or cron
budget overrun — the current pipeline sends a raw, technical message directly
to Telegram:

```
⚠️ Agent timed out (600s safety limit). Last activity:
─────────────
💭 thinking...
🔧 mcp__right__notion__notion-fetch <code>{"id":"..."}</code>
🔧 mcp__right__notion__notion-fetch <code>{"id":"..."}</code>
...
Stream log: ~/.rightclaw/logs/streams/eaad18de-....ndjson
```

The user sees a log dump and a filesystem path. The agent — which has the full
context of what it was investigating, partial findings, and understanding of
the user's intent — never gets a chance to produce a human-friendly summary.

Cron jobs suffer a worse variant: on failure (timeout, budget exceeded) the
`notify_json` is often empty, so `cron_delivery` doesn't even deliver anything.
The user silently gets no notification that their cron failed.

## Goal

On failure, give the agent one short chance to reply to the user with a
human-friendly summary of what it was doing and what it learned, instead of
surfacing raw platform errors.

## Scope

**In scope (worker):**
- Safety timeout (600s)
- Non-zero exit code (excluding auth errors, which go through login flow)

**In scope (cron):**
- Safety timeout
- Budget exceeded
- Max turns reached
- Non-zero exit code

**Out of scope:**
- Parse failure (CC returned stdout that does not match reply schema) — already
  has raw content as a reasonable fallback, reflection unlikely to improve it.
- Auth errors — already handled by the login flow.
- `cron_delivery` own failures — second-order, no useful context to reflect on.
- Reflection-of-reflection — hard stop. If reflection itself fails, fallback
  path sends the raw failure message.

## Design

### Reflection Primitive

A new module `crates/bot/src/reflection.rs` exposes one public function:

```rust
pub async fn reflect_on_failure(ctx: ReflectionContext) -> Result<String, ReflectionError>;

pub struct ReflectionContext {
    pub session_uuid: String,
    pub failure: FailureKind,
    pub ring_buffer_tail: Vec<StreamEvent>,
    pub stream_log_path: Option<PathBuf>,
    pub limits: ReflectionLimits,
    pub agent_dir: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub db_path: PathBuf,
    pub parent_source: ParentSource,
}

pub enum FailureKind {
    SafetyTimeout,
    BudgetExceeded,
    MaxTurns,
    NonZeroExit { code: i32 },
}

pub enum ParentSource {
    Worker { chat_id: i64, thread_id: i64 },
    Cron   { job_name: String },
}

pub struct ReflectionLimits {
    pub max_turns: u32,
    pub max_budget_usd: f64,
    pub process_timeout: Duration,
}

impl ReflectionLimits {
    pub const WORKER: Self = Self { max_turns: 3, max_budget_usd: 0.20, process_timeout: Duration::from_secs(90)  };
    pub const CRON:   Self = Self { max_turns: 5, max_budget_usd: 0.40, process_timeout: Duration::from_secs(180) };
}
```

The primitive:
1. Builds the SYSTEM_NOTICE prompt (see below).
2. Constructs a `ClaudeInvocation` with `--resume <session_uuid>`,
   `--output-format stream-json`, `--max-turns`, `--max-budget-usd`,
   `--disallowedTools Agent`, and the standard reply JSON schema.
3. Invokes `claude -p` via the same path as worker (SSH into sandbox if
   configured, direct exec otherwise), wrapped in `tokio::time::timeout`.
4. Parses the final `result` stream event, extracts the reply text and usage
   breakdown.
5. Calls `insert_reflection(...)` into `usage_events` with
   `source = "reflection"` and the appropriate discriminator fields.
6. Returns the reply text.

On any failure (timeout, non-zero exit, parse failure) returns
`Err(ReflectionError)` — the caller decides fallback.

### SYSTEM_NOTICE Prompt

Input text piped to `claude -p` stdin during reflection:

```
⟨⟨SYSTEM_NOTICE⟩⟩

Your previous turn did not complete successfully.

Reason: <human-readable reason per FailureKind>

Your most recent activity:
<ring buffer tail rendered as bulleted list — tool names + short args, text snippets truncated to 80 chars>

Please write a short reply for the user that:
1. Acknowledges the interruption honestly (1 sentence).
2. Summarizes what you were doing and any findings worth sharing.
3. Suggests a concrete next step (narrower scope, different approach,
   or ask for clarification).

Do NOT continue the original investigation — stay within 3 turns.
Do NOT call Agent or other long-running tools.
⟨⟨/SYSTEM_NOTICE⟩⟩
```

FailureKind → reason text:
- `SafetyTimeout` → `"hit the 600-second safety limit before producing a reply"`
- `BudgetExceeded` → `"exceeded the budget of $X"` (numeric value interpolated)
- `MaxTurns` → `"reached the maximum turn count (N)"`
- `NonZeroExit { code }` → `"Claude process exited with code N"`

### OPERATING_INSTRUCTIONS Update

A new block is appended to `OPERATING_INSTRUCTIONS` in
`crates/rightclaw/src/codegen/agent_def.rs`. The block is static (compiled in,
does not depend on agent config) and teaches the agent how to handle SYSTEM_NOTICE
messages on the *current* turn and on all subsequent turns when the session
resumes:

```markdown
## System Notices

Some of your incoming messages may be wrapped in ⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩.
These are platform-generated — not user messages. They appear when the platform
needs to inform you of something about your own prior execution (a timeout,
a budget cap, an exit failure, etc.) and ask you to respond with a user-facing
summary.

Rules:
- Follow the instructions inside the notice for the current turn.
- Do NOT quote the ⟨⟨SYSTEM_NOTICE⟩⟩ marker in your reply.
- On subsequent turns, do NOT treat the notice as if the user sent it —
  the user did not see it. They only see your reply.
- Do NOT reflect on, apologize for, or reference the notice in later turns
  unless the user explicitly asks about what happened.
```

This invalidates the prompt cache for all agents once when the new code ships.
No runtime overhead.

### Worker Integration

Refactor `invoke_cc` return type (crates/bot/src/telegram/worker.rs):

```rust
// Before:
async fn invoke_cc(...) -> Result<(Option<ReplyOutput>, String), String>;

// After:
async fn invoke_cc(...) -> Result<(Option<ReplyOutput>, String), InvokeCcFailure>;

pub enum InvokeCcFailure {
    Reflectable {
        kind: FailureKind,
        ring_buffer_tail: Vec<StreamEvent>,
        session_uuid: String,
        stream_log_path: Option<PathBuf>,
        raw_message: String,
    },
    NonReflectable {
        message: String,
    },
}
```

Call sites that currently `return Err(timeout_msg)` or
`return Err(format_error_reply(...))` produce `Reflectable` variants. Parse
failures and internal errors stay `NonReflectable`. Auth errors continue to
return `Ok((None, session_uuid))` (unchanged).

In `spawn_worker` after `invoke_cc`:

```rust
match invoke_cc_result {
    Ok((output, uuid)) => { /* existing success path */ }
    Err(InvokeCcFailure::NonReflectable { message }) => {
        send_to_telegram(message);
    }
    Err(InvokeCcFailure::Reflectable { kind, ring_buffer_tail, session_uuid, stream_log_path, raw_message }) => {
        match reflection::reflect_on_failure(ReflectionContext {
            session_uuid: session_uuid.clone(),
            failure: kind,
            ring_buffer_tail,
            stream_log_path,
            limits: ReflectionLimits::WORKER,
            agent_dir: ctx.agent_dir.clone(),
            ssh_config_path: ctx.ssh_config_path.clone(),
            resolved_sandbox: ctx.resolved_sandbox.clone(),
            db_path: ctx.db_path.clone(),
            parent_source: ParentSource::Worker { chat_id, thread_id: eff_thread_id },
        }).await {
            Ok(reply_text) => send_to_telegram(reply_text),
            Err(reflection_err) => {
                tracing::warn!(?chat_id, "reflection failed: {reflection_err:#}; sending raw error");
                send_to_telegram(raw_message);
            }
        }
    }
}
```

Session stays active — the next user message `--resume`s into the same session,
where the agent sees the SYSTEM_NOTICE → reflection reply sequence and (thanks
to OPERATING_INSTRUCTIONS) correctly treats SYSTEM_NOTICE as non-user input.

### Worker UX — Thinking Message Flow

1. Original thinking message is finalized as `⚠️ Hit 600-second limit`
   (short, neutral — no ring buffer dump, no log path).
2. If `show_thinking: true && !is_group`, a new thinking message is created
   titled `⏳ Thinking again…` with its own `CancellationToken`. Live events
   from the reflection session are streamed into it using the existing
   `format_thinking_message` infrastructure.
3. On reflection success: `send_message` with the reply.
4. On reflection failure / cancellation: `send_message` with `raw_message`.

In group chats or with `show_thinking: false`: no second thinking message, just
the final reply.

### Cron Integration

Current `cron.rs` on failure records `status='failed'` but often leaves
`notify_json = NULL`, so `cron_delivery` never picks it up.

New flow — `run_cron_spec` on detected failure:
1. Record raw failure immediately (`status='failed'`, `notify_json=NULL`,
   `log_path`, `summary` with reason).
2. Call `reflection::reflect_on_failure` inline with
   `ReflectionLimits::CRON` and `ParentSource::Cron { job_name }`.
3. If reflection succeeds → `UPDATE cron_runs SET notify_json = <JSON with
   reflection text as content>` where row id matches.
4. If reflection fails → `UPDATE cron_runs SET notify_json = <JSON with raw
   failure summary as content>` (degraded but non-empty). The user still learns
   the cron failed.

### Cron Delivery Branching

`cron_delivery.rs` currently uses one `DELIVERY_INSTRUCTION` with "verbatim"
semantics (good for success, wrong for failures because the reflection text is
written in first person by the agent and expected to be relayed with light
contextual framing).

Two instruction constants:

```rust
const DELIVERY_INSTRUCTION_SUCCESS: &str = /* existing */;

const DELIVERY_INSTRUCTION_FAILURE: &str = "\
The cron job below did not complete successfully. The `content` field contains
a platform-generated summary of the failure. Relay it to the user in natural
prose — you MAY rephrase lightly for flow with recent conversation, but keep
all factual claims intact. Do not invent details.
Ignore the attachments field.

Here is the YAML report:
";
```

Selection in `format_cron_yaml`:
- Add `status: String` to `PendingCronResult`.
- Update `fetch_pending` SQL to `SELECT ... status FROM cron_runs ...`.
- Pick instruction based on `pending.status`.

`CronNotify` struct is unchanged — the `content` field serves both cases.

### Usage Accounting

New value `"reflection"` for `usage_events.source` — no DDL migration needed.

New helper in `crates/rightclaw/src/usage/insert.rs`:

```rust
pub fn insert_reflection(
    conn: &Connection,
    b: &UsageBreakdown,
    parent: &ParentSource,
) -> Result<(), UsageError> {
    match parent {
        ParentSource::Worker { chat_id, thread_id } =>
            insert_row(conn, b, "reflection", Some(*chat_id), Some(*thread_id), None),
        ParentSource::Cron { job_name } =>
            insert_row(conn, b, "reflection", None, None, Some(job_name.as_str())),
    }
}
```

Discriminator recovery for `/usage detail`: a reflection row with `chat_id IS
NOT NULL` came from the worker path; with `job_name IS NOT NULL` came from
the cron path.

`aggregate.rs::aggregate()` is a free function that takes `source: &str` — no
change. `crates/rightclaw/src/usage/format.rs::AllWindows` gains fields:

```rust
pub struct AllWindows {
    // ... existing interactive/cron fields ...
    pub today_reflection: Summary,
    pub week_reflection:  Summary,
    pub month_reflection: Summary,
    pub all_reflection:   Summary,
}
```

`format_summary_message` adds a "Reflection: $X.XX (Y turns)" line for each
window. In `detail` mode, reflection is split into "from worker" vs "from cron"
using the discriminator recovery above.

`build_usage_summary` in `crates/bot/src/telegram/handler.rs` calls
`aggregate(&conn, window, "reflection")` for each window.

### Observability

- `tracing::instrument` on `reflect_on_failure` with fields
  `parent_source`, `failure_kind`, `session_uuid`. Structured INFO-level logs
  on start and on completion (duration_ms, cost_usd, turns, fallback: bool).
- Reflection stream events are written to the same per-session NDJSON file as
  the parent session, tagged with `"turn_kind": "reflection"`. No new log
  files. Full timeline is preserved.
- Optional `/doctor` check `reflection_health` (MVP-optional): warns if
  reflection cost in the last 24h exceeds 10% of interactive+cron cost.

### Invariants

- Reflection never triggers reflection. If `reflect_on_failure` fails, the
  caller falls back to the raw failure message. No recursion.
- Reflection does not write to Hindsight (`memory_retain` is skipped).
  Rationale: SYSTEM_NOTICE prompts are platform noise, not user-agent
  conversation. Including them would pollute the memory bank.
- Reflection for cron is written inline in `run_cron_spec`, not spawned
  separately — the cron scheduler waits for the reflection to complete before
  moving on. This keeps `cron_runs` rows consistent (no "failed with NULL
  notify_json" intermediate state visible to delivery).
- `cron_delivery`'s own failures do NOT reflect — second-order failure with
  no useful context.
- All reflection `claude -p` calls go through `ClaudeInvocation` (the existing
  invariant from ARCHITECTURE.md "Claude Invocation Contract").

## Testing

Unit tests (`reflection.rs`):
- Prompt builder renders each `FailureKind` correctly.
- Ring buffer tail is truncated and formatted.
- `ReflectionLimits::WORKER` and `::CRON` constants are exercised.

Integration tests (live sandbox via `TestSandbox`):
- Worker timeout → reflection → reply received in Telegram-facing channel.
- Worker non-zero exit → reflection → reply.
- Reflection itself times out → fallback to raw message.
- Cron budget exceeded → reflection → `notify_json` populated →
  `cron_delivery` picks up and delivers with `DELIVERY_INSTRUCTION_FAILURE`.
- `insert_reflection` writes correct `source` + discriminator for both parents.
- `/usage` renders Reflection line; `/usage detail` splits worker vs cron.

## Files Changed

```
NEW:  crates/bot/src/reflection.rs                             (new module)
MOD:  crates/bot/src/lib.rs                                    (pub mod reflection)
MOD:  crates/bot/src/telegram/worker.rs                        (invoke_cc return type, reflectable match arm, thinking UX)
MOD:  crates/bot/src/cron.rs                                   (reflection call on failure, notify_json update)
MOD:  crates/bot/src/cron_delivery.rs                          (status field in PendingCronResult, two DELIVERY_INSTRUCTIONs)
MOD:  crates/rightclaw/src/codegen/agent_def.rs                (OPERATING_INSTRUCTIONS += System Notices block)
MOD:  crates/rightclaw/src/usage/insert.rs                     (insert_reflection helper + ParentSource)
MOD:  crates/rightclaw/src/usage/format.rs                     (AllWindows += reflection fields, format_summary)
MOD:  crates/bot/src/telegram/handler.rs                       (build_usage_summary += reflection aggregates)
MOD:  crates/rightclaw/src/doctor.rs                           (reflection_health check — optional)
MOD:  ARCHITECTURE.md                                          (document reflection primitive + SYSTEM_NOTICE)
MOD:  PROMPT_SYSTEM.md                                         (document SYSTEM_NOTICE convention)
```

## Alternatives Considered

**New session (no --resume) for reflection.** Rejected — loses full context
(tool results, memory, attachments). Agent would only have the ring buffer
tail (tool names, no outputs) to summarize from. Weak answer.

**Feed raw error as fake user message.** Rejected — agent would interpret it
as a real user message and potentially try to investigate the timeout itself
instead of summarizing. SYSTEM_NOTICE marker + explicit prefix avoids this.

**Include parse failure in scope.** Rejected — parse failure means CC returned
text that didn't match the schema. The raw stdout is often a reasonable
message; reflecting on it adds cost without clear benefit.

**Deactivate session after reflection.** Rejected — loses continuity. User
might follow up with "ok, try a narrower scope" and the agent would have no
context. SYSTEM_NOTICE marker + OPERATING_INSTRUCTIONS handle the "don't
treat it as user input on next turn" concern.

**Two new sources (`reflection_worker`, `reflection_cron`).** Rejected — one
source with discriminator via existing `chat_id`/`job_name` columns is
simpler and keeps aggregation queries uniform.

**New `reflection_text` column in `cron_runs`.** Rejected — `notify_json.content`
serves the purpose, delivery branching is based on `status` which is already
there. No migration.

**Configurable reflection on/off per agent (`reflection: true` in agent.yaml).**
Rejected for MVP — YAGNI. Reflection is strictly an improvement over raw
error dumps; if it becomes costly or undesirable later, add the toggle then.

## Risks

- **Prompt cache invalidation on deploy.** Adding the System Notices block to
  OPERATING_INSTRUCTIONS invalidates all agents' prompt cache once. Acceptable
  one-time cost.
- **Reflection cost.** Each failure now costs an additional short CC invocation
  ($0.05–0.20 worker, $0.10–0.40 cron). If failures are frequent, this adds up.
  Mitigation: `/doctor reflection_health` warns at 10% threshold.
- **Agent misuse of SYSTEM_NOTICE on subsequent turns.** Despite OPERATING_INSTRUCTIONS,
  an agent might still reference "that time I timed out" in later replies.
  Acceptable — not a correctness issue, just occasional mild awkwardness.
- **Cron reflection delays cron_runs visibility.** Between failure and reflection
  completion, the row has `status='failed'` + `notify_json=NULL`. If
  `cron_delivery` happens to poll in that window, it skips the row (existing
  filter). On next poll (30s) the row has notify_json and gets delivered.
  No bug, just a small delivery delay bounded by reflection process timeout.
