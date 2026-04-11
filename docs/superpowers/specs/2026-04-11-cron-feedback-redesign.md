# Cron Feedback Redesign

## Problem

Cron results are sent directly to Telegram via `bot.send_message()`, bypassing the main CC session. When a user replies to a cron notification, the bot loses context — the main session has no knowledge of what the cron produced. The user cannot continue a conversation about cron results.

## Solution

Deliver cron results **through the main CC session** (`claude -p`). The agent gets full context, and the user can reply naturally.

## New Cron Structured Output

Replace `REPLY_SCHEMA_JSON` for crons with a dedicated schema:

```json
{
  "type": "object",
  "properties": {
    "notify": {
      "type": ["object", "null"],
      "properties": {
        "content": { "type": "string" },
        "attachments": {
          "type": ["array", "null"],
          "items": {
            "type": "object",
            "properties": {
              "type": { "enum": ["photo", "document", "video", "audio", "voice", "video_note", "sticker", "animation"] },
              "path": { "type": "string" },
              "filename": { "type": ["string", "null"] },
              "caption": { "type": ["string", "null"] }
            },
            "required": ["type", "path"]
          }
        }
      },
      "required": ["content"]
    },
    "summary": { "type": "string" }
  },
  "required": ["summary"]
}
```

- `notify: null` — cron ran silently, nothing to send to user
- `notify.content` — delivered through the main session
- `summary` — always present, persisted to DB

## DB Schema — `cron_runs` Extension

New columns added to existing `cron_runs` table:

```sql
ALTER TABLE cron_runs ADD COLUMN summary TEXT;
ALTER TABLE cron_runs ADD COLUMN notify_json TEXT;      -- JSON or NULL
ALTER TABLE cron_runs ADD COLUMN delivered_at TEXT;      -- NULL until delivered
```

- `summary` — always written on successful completion
- `notify_json` — serialized `notify` object as JSON string (e.g. `{"content":"...","attachments":[...]}`), NULL when cron is silent
- `delivered_at` — set when result is delivered through main session; key field for the poll loop

## Delivery Poll Loop

A dedicated tokio task runs alongside the existing cron engine:

1. **Idle detection**: wait for 5 minutes of inactivity. Idle timer resets on any interaction — user message received OR bot reply sent (including cron delivery replies).

2. **Query pending results**:
   ```sql
   SELECT * FROM cron_runs
   WHERE status = 'success'
     AND notify_json IS NOT NULL
     AND delivered_at IS NULL
   ORDER BY finished_at ASC
   LIMIT 1
   ```

3. **Deduplicate**: before delivering, mark all older undelivered results of the same `job_name` as delivered (set `delivered_at = NOW()`) without actual delivery. Only the latest result per job gets delivered.

4. **Format YAML** and pipe into `claude -p` of the main session:
   ```yaml
   cron_result:
     job: health-check
     runs_total: 3
     skipped_runs: 2
     result:
       notify:
         content: "BTC broke 100k"
         attachments:
           - type: photo
             path: /home/user/.rightclaw/agents/trader/outbox/cron/abc123/chart.png
       summary: "Checked 5 pairs, 1 alert triggered"
   ```

5. **Agent responds** using the standard `REPLY_SCHEMA_JSON` (content + attachments). Worker sends to Telegram as usual.

6. **Mark delivered**: set `delivered_at = NOW()` on the cron_run row.

7. **Loop**: if user is still idle, check for next pending cron result. Any interaction resets the 5-minute idle timer.

## Idle Timer

Tracks the timestamp of the last interaction. An "interaction" is:

- User sends a message (any message received by handler)
- Bot finishes sending a reply (worker completes a `claude -p` call, including cron delivery)

The poll loop checks `now - last_interaction >= 5 minutes` before each delivery attempt. If the user becomes active during delivery, the loop pauses and waits for idle again.

## Attachment Handling

### Sandbox mode
1. Cron completes → rightclaw downloads attachments from sandbox to `agent_dir/outbox/cron/{run_id}/` on host
2. `notify_json` stores host-side paths (not sandbox paths)
3. Poll loop passes host paths in YAML to main session
4. Main session references paths in its `REPLY_SCHEMA_JSON` response
5. Worker sends attachments to Telegram
6. Cleanup: after `delivered_at` is set, delete `outbox/cron/{run_id}/`

### No-sandbox mode
Skip step 1 — files are already on host. Store original paths in `notify_json`.

## Changes Summary

| Component | Before | After |
|-----------|--------|-------|
| Cron structured output | `REPLY_SCHEMA_JSON` (content, attachments, reply_to_message_id) | New schema (notify, summary) |
| Delivery | `bot.send_message()` directly | Through main `claude -p` session |
| Session context | Outside session, user cannot reply | In session, user continues conversation |
| Concurrency | Fire-and-forget | 5-min idle + sequential delivery |
| Persistence | `cron_runs` (exit_code, status, log_path) | + summary, notify_json, delivered_at |
| Attachments | Not supported in crons | outbox/cron/{run_id}/ + cleanup after delivery |
| Deduplication | None | Latest result per job_name wins; older skipped |

## Files to Modify

| File | Change |
|------|--------|
| `crates/bot/src/cron.rs` | Replace `bot.send_message` with DB persist; download attachments from sandbox; new structured output parsing |
| `crates/rightclaw/src/codegen/agent_def.rs` | Add `CRON_SCHEMA_JSON` constant |
| `crates/rightclaw/src/memory/migrations.rs` | New migration: add summary, notify_json, delivered_at columns |
| `crates/rightclaw/src/memory/sql/` | New migration SQL file |
| `crates/bot/src/cron.rs` (or new `cron_delivery.rs`) | Poll loop task: idle detection, DB query, YAML formatting, `claude -p` invocation |
| `crates/bot/src/telegram/worker.rs` | Expose idle timestamp (shared `Arc<AtomicI64>` or similar); accept cron delivery as input |
| `crates/bot/src/telegram/handler.rs` | Update idle timestamp on incoming messages |
| `crates/bot/src/lib.rs` | Wire up delivery poll loop task; share idle state between worker/handler/delivery |

## Out of Scope

- Urgency levels / priority delivery
- Cron result aggregation across multiple jobs
- Cron failure notifications (failures are logged, not delivered)
