# Cron Self-Diagnostics

## Problem

The agent cannot diagnose cron delivery issues. `cron_run_to_json` omits `delivered_at`, and CC never explains why it chose `notify: null`. When asked "why didn't you notify me?", the agent guesses instead of reporting facts.

## Changes

### 1. CC-level: `no_notify_reason` in cron structured output

Add `no_notify_reason` to `CRON_SCHEMA_JSON` and `CronReplyOutput`:

```rust
pub struct CronReplyOutput {
    pub notify: Option<CronNotify>,
    pub summary: String,
    pub no_notify_reason: Option<String>,
}
```

**Schema rule:** required when `notify` is null, omitted when `notify` is present. CC fills it with a short factual explanation ("No changes since last run", "All repos quiet, no new releases or issue activity").

Persisted to a new `no_notify_reason TEXT` column in `cron_runs`. When `notify` is present, stored as NULL.

### 2. Platform-level: `delivery_status` lifecycle tracking

New `delivery_status TEXT` column in `cron_runs`, set at different lifecycle points:

| Value | Set when | Meaning |
|-------|----------|---------|
| `silent` | Cron execution completes with `notify` null | CC decided nothing to report |
| `pending` | Cron execution completes with `notify` present | Awaiting delivery through main session |
| `delivered` | Delivery loop succeeds | Sent to Telegram via main CC session |
| `superseded` | Dedup in delivery loop | Newer run for same job replaced this one |
| `failed` | Max retry attempts exhausted (3) | Delivery gave up after retries |

### 3. Expose in MCP output

Extend `cron_run_to_json` to include three new fields:

- `delivered_at` — already in DB, not currently exposed
- `delivery_status` — new column (see above)
- `no_notify_reason` — new column (see above)

Agent sees:
```json
{
  "id": "59ab66d3",
  "status": "success",
  "delivery_status": "silent",
  "no_notify_reason": "No changes since last run — same issues, same releases",
  "delivered_at": null
}
```

## Files modified

| File | Change |
|------|--------|
| `crates/rightclaw/src/codegen/agent_def.rs` | Update `CRON_SCHEMA_JSON` — add `no_notify_reason` field |
| `crates/rightclaw/src/memory/sql/v12_cron_diagnostics.sql` | Migration: add `delivery_status TEXT`, `no_notify_reason TEXT` |
| `crates/rightclaw/src/memory/migrations.rs` | Register v12 migration |
| `crates/bot/src/cron.rs` | Parse + persist `no_notify_reason`; set `delivery_status` to `silent` or `pending` |
| `crates/bot/src/cron_delivery.rs` | Set `delivery_status` to `delivered`, `superseded`, or `failed` |
| `crates/rightclaw-cli/src/memory_server.rs` | Extend `cron_run_to_json` with `delivered_at`, `delivery_status`, `no_notify_reason` |
| `crates/rightclaw-cli/src/right_backend.rs` | Update SQL queries to SELECT new columns |
| `skills/rightcron/SKILL.md` | Document new fields in run records |

## Migration SQL

```sql
ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT;
ALTER TABLE cron_runs ADD COLUMN no_notify_reason TEXT;

-- Backfill existing rows:
-- Runs with notify_json and delivered_at → delivered
UPDATE cron_runs SET delivery_status = 'delivered'
  WHERE notify_json IS NOT NULL AND delivered_at IS NOT NULL;
-- Runs with notify_json but no delivered_at → pending
UPDATE cron_runs SET delivery_status = 'pending'
  WHERE notify_json IS NOT NULL AND delivered_at IS NULL;
-- Runs without notify_json → silent
UPDATE cron_runs SET delivery_status = 'silent'
  WHERE notify_json IS NULL;
```

## Schema change (CRON_SCHEMA_JSON)

Current:
```json
{
  "type": "object",
  "properties": {
    "notify": { ... },
    "summary": { "type": "string" }
  },
  "required": ["summary"]
}
```

New — add `no_notify_reason`:
```json
{
  "type": "object",
  "properties": {
    "notify": { ... },
    "summary": { "type": "string" },
    "no_notify_reason": { "type": ["string", "null"] }
  },
  "required": ["summary"]
}
```

CC is instructed (via the schema comment in `agent_def.rs` and cron skill) that `no_notify_reason` is required when `notify` is null. JSON Schema can't enforce conditional requires cleanly, so this is a convention enforced by the doc comment + skill text.

## Not in scope

- New MCP tools (no `cron_diagnose` — data exposure is sufficient)
- Changes to delivery loop timing or idle threshold
- Changes to dedup logic
