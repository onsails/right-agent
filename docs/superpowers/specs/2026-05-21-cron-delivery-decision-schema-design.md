# Cron Delivery Decision Schema

**Date:** 2026-05-21
**Status:** Design approved; pending implementation plan

## Problem

A scheduled reminder cron can complete successfully without notifying the user.
The observed production run on 2026-05-20 produced the reminder text as normal
assistant stream output, then returned structured output containing only
`summary`. The runtime only delivers structured `notify_json`, so the run was
persisted as successful but non-delivering:

- `delivery_required = 0`
- `delivery_status = 'none'`
- `notify_json = NULL`
- `summary = "Drafted a short Russian Telegram reminder..."`

This is a platform contract bug. The current cron schema requires `summary` but
makes `notify` optional, and the prompt does not make it impossible to confuse
ordinary assistant text with the user-facing delivery channel.

## Goals

- Make every cron result choose an explicit delivery decision.
- Preserve valid silent crons for conditional monitors and "nothing to report"
  jobs.
- Make reminder/message/ping crons put user-facing text in the structured
  delivery payload.
- Rename `summary` to a technical name that does not imply user-facing content.
- Keep migration simple and lossy; old cron delivery data may be discarded.
- Keep `PROMPT_SYSTEM.md` in sync with the real prompt/schema contract.

## Non-goals

- No boolean-only `notify` flag. A delivery decision must include either the
  payload to deliver or the reason for silence.
- No complex Rust migration hook to preserve old structured payloads.
- No attempt to replay or recover old pending deliveries.
- No change to the idle delivery gate semantics in this design.
- No sandbox recreation or manual state repair.

## Decision

Use an explicit delivery decision object:

```json
{
  "delivery": {
    "kind": "notify",
    "content": "User-facing Telegram text",
    "attachments": null
  },
  "run_note": "Technical note for logs/history"
}
```

Silent cron result:

```json
{
  "delivery": {
    "kind": "silent",
    "reason": "No relevant changes since the previous run"
  },
  "run_note": "Checked the configured source; no threshold crossed"
}
```

`run_note` replaces `summary`. It is technical metadata for history/debugging,
not a delivery channel.

## Schema

`CRON_SCHEMA_JSON` becomes a required `delivery` decision plus required
`run_note`:

```json
{
  "type": "object",
  "properties": {
    "delivery": {
      "oneOf": [
        {
          "type": "object",
          "properties": {
            "kind": { "const": "notify" },
            "content": { "type": "string", "minLength": 1 },
            "attachments": {
              "type": ["array", "null"],
              "items": {
                "type": "object",
                "properties": {
                  "type": {
                    "enum": [
                      "photo",
                      "document",
                      "video",
                      "audio",
                      "voice",
                      "video_note",
                      "sticker",
                      "animation"
                    ]
                  },
                  "path": { "type": "string" },
                  "filename": { "type": ["string", "null"] },
                  "caption": { "type": ["string", "null"] }
                },
                "required": ["type", "path"]
              }
            }
          },
          "required": ["kind", "content"]
        },
        {
          "type": "object",
          "properties": {
            "kind": { "const": "silent" },
            "reason": { "type": "string", "minLength": 1 }
          },
          "required": ["kind", "reason"]
        }
      ]
    },
    "run_note": { "type": "string" }
  },
  "required": ["delivery", "run_note"]
}
```

The design relies on `oneOf + const + minLength`. A live Claude Code probe with
Haiku accepted this shape and produced structurally valid outputs for normal
notify and silent prompts. A flat enum schema was less reliable: it allowed
branch leakage and produced invalid silent objects in the probe.

The schema improves structural correctness, but it cannot prove task intent. An
adversarial "write a reminder but only summarize it" prompt can still choose a
valid `silent` branch. The prompt and runtime validation must carry the semantic
contract.

## Runtime Parsing

Replace the optional `notify` parsing model with a Rust enum equivalent to:

```rust
enum CronDeliveryDecision {
    Notify(CronNotify),
    Silent { reason: String },
}
```

Runtime validation rules:

- `Notify.content.trim()` must be non-empty.
- `Silent.reason.trim()` must be non-empty.
- Missing `delivery`, unknown `kind`, malformed branch payload, or empty
  branch text is invalid cron output.
- Invalid cron output is not treated as silent. The run should surface as a
  failed/invalid result with enough log detail to debug the model output.

Persistence derives queue fields from the parsed enum:

- `Notify` stores the full delivery decision JSON, sets
  `delivery_required = 1`, and sets `delivery_status = 'pending'`.
- `Silent` stores the full delivery decision JSON, sets
  `delivery_required = 0`, and sets `delivery_status = 'none'`.

## Storage

Rename async run columns to match the new contract:

```text
run_note          TEXT
delivery_json     TEXT
delivery_required INTEGER NOT NULL
delivery_status   TEXT NOT NULL
```

Remove:

```text
summary
notify_json
no_notify_reason
```

`delivery_json` stores the full delivery decision object for new rows. Notify
rows carry `kind = "notify"` plus user-facing content; silent rows carry
`kind = "silent"` plus the reason. `delivery_required` and `delivery_status`
remain runtime queue/cache fields, not model-authored fields.

## Migration

Use a SQL-only, lossy migration. Existing async run rows are migrated as:

- `run_note = old summary`
- `delivery_json = NULL`
- `delivery_required = 0`
- `delivery_status = 'none'` for old rows that were not already terminally
  delivered/superseded/failed

Old pending deliveries may be lost. That is acceptable for this change and is
preferable to a complex Rust migration hook that tries to translate historical
cron payloads.

The implementation plan should verify the exact existing `async_runs` schema
before writing the migration SQL. If the current table already contains
terminal delivery states that must be kept for history, the migration may
preserve those status strings while still clearing delivery payloads.

## Prompt Contract

Update cron instructions so the model gets the same contract the runtime
enforces:

- Ordinary assistant text in a cron turn is not delivered to Telegram.
- `delivery.kind = "notify"` is the only user-facing cron delivery path.
- `delivery.content` is the Telegram text.
- `run_note` is technical metadata for history/debugging and is never
  user-facing.
- Reminder, ping, tag, tell, message, and notify tasks must choose
  `delivery.kind = "notify"` unless the task is explicitly conditional and the
  condition was not met.
- `delivery.kind = "silent"` is valid for monitors, checks, and conditional
  tasks only when there is factually nothing to report or delivery is blocked by
  a real condition. The reason must be factual and short.

The wording should avoid saying "summary" anywhere in the new contract.

## Data Flow

1. Scheduler starts a cron run and passes the new JSON schema to Claude Code.
2. Cron prompt plus system instructions tell the agent to put the user-facing
   result in `delivery.content` when notification is intended.
3. Runtime parses the final structured output into the delivery decision enum.
4. Runtime validates the branch-specific fields.
5. Runtime persists `run_note`, optional `delivery_json`, and derived queue
   fields into `async_runs`.
6. Async delivery loop reads only rows with required pending delivery and
   `delivery_json` whose parsed decision is `kind = "notify"`.
7. Silent rows remain visible in run history but never enter the delivery queue.

## Files Expected To Change

- `crates/right-codegen/src/agent_def.rs`
  - update `CRON_SCHEMA_JSON`
  - keep any prompt/schema snapshots consistent
- `crates/right-codegen/templates/right/prompt/CRON_INSTRUCTIONS.md`
  - update the cron delivery contract text
- `PROMPT_SYSTEM.md`
  - update generated prompt documentation to match the schema and instructions
- `crates/bot/src/cron.rs`
  - replace optional notify output parsing with delivery decision parsing
  - derive persistence fields from the enum
  - treat missing/invalid delivery as invalid output
- `crates/bot/src/async_delivery.rs`
  - rename queue payload reads from `notify_json` to `delivery_json`
- `crates/right-agent/src/async_runs.rs`
  - rename structs and SQL bindings from summary/notify to run_note/delivery
- `crates/right-db/src/sql/*.sql`
  - add a SQL-only migration for the lossy column transition
- `crates/right-db/src/migrations.rs`
  - register the SQL migration only; no custom Rust hook
- `docs/architecture/*.md`
  - cite-on-touch update if cron/async run architecture docs drift

## Error Handling

- Missing `delivery`: invalid output, not silent.
- `delivery.kind = "notify"` with empty/whitespace content: invalid output,
  not silent.
- `delivery.kind = "silent"` with empty/whitespace reason: invalid output.
- Unknown delivery kind: invalid output.
- Delivery loop row with `delivery_required = 1` but null/invalid
  `delivery_json`, or with a non-notify decision: mark delivery
  failed/retryable according to the existing delivery error policy; do not
  synthesize a message from `run_note`.

The important invariant is that `run_note` is never used as fallback Telegram
content.

## Testing And Verification

Use TDD for behavior changes in the implementation plan.

Targeted regression tests:

- A cron structured output with `delivery.kind = "notify"` persists the full
  delivery decision JSON, `delivery_required = 1`, and
  `delivery_status = 'pending'`.
- A cron structured output with `delivery.kind = "silent"` persists the full
  delivery decision JSON, `delivery_required = 0`, and does not enter the
  delivery queue.
- Missing `delivery` is rejected as invalid output.
- Empty notify content and empty silent reason are rejected.
- The async delivery query reads `delivery_json` and never falls back to
  `run_note`.
- Migration test covers the lossy rename/drop behavior.
- Prompt/schema snapshot tests cover `CRON_SCHEMA_JSON`,
  `CRON_INSTRUCTIONS.md`, and `PROMPT_SYSTEM.md`.

Verification cadence:

- Start implementation with the narrowest relevant baseline test/check for the
  touched crates.
- Run targeted package/module tests after the parser, persistence, delivery,
  and migration slices.
- Before claiming implementation complete, run
  `devenv shell -- cargo test --workspace`.
- Final Rust build required by project convention:
  `devenv shell -- cargo build --workspace`.

## Acceptance Criteria

- A cron cannot successfully return structured output without an explicit
  delivery decision.
- Reminder-style crons are instructed to put the reminder in
  `delivery.content`.
- Silent cron runs remain supported and explicit.
- The database and Rust naming no longer use `summary` for cron run notes.
- Old pending cron delivery payloads may be discarded by migration.
- No complex Rust migration hook is introduced.
