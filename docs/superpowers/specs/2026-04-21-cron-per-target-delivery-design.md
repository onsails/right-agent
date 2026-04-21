# Per-Cron Delivery Target — Design

**Status:** Draft
**Author:** Andrey Kuznetsov
**Date:** 2026-04-21

## Problem

A cron job created in a Telegram group does not deliver its result back to that group. It is delivered to the agent owner's DM instead.

### Root cause (two layers)

**Layer 1 — wrong source for delivery targets.**
`crates/bot/src/lib.rs:416` and `:638` build `notify_chat_ids` / `delivery_chat_ids` from `config.allowed_chat_ids` — the legacy field in `agent.yaml`. The new bot-managed `allowlist.yaml` (introduced by `2026-04-16-group-chat-design`) is the source of truth for routing, but cron delivery never reads it. Any group added via `/allow_all` after the agent was first configured is invisible to cron delivery.

**Layer 2 — no per-cron addressing.**
The `cron_specs` table (`crates/rightclaw/src/memory/sql/v6_cron_specs.sql`) has no chat columns, and the `cron_create` MCP tool (`crates/rightclaw-cli/src/right_backend.rs:154`) takes no chat arguments. Crons have no concept of "where I was created" or "where I should deliver." The current delivery loop fans out to every chat in `notify_chat_ids` instead of one specific chat.

Even if Layer 1 were fixed in isolation, an agenda cron created in one group would still be broadcast to every trusted DM and every open group.

### Concrete example

Agent `him` has:
- `agent.yaml` → `allowed_chat_ids: [12345678]` (owner DM only)
- `allowlist.yaml` → users `[12345678]` + groups `[-1001234567890 "aibots"]` (group opened 2026-04-21 11:41:38Z)
- one cron (`agenda`) created from a message in the `aibots` group

When the cron fires, `cron_delivery::deliver_through_session` iterates `notify_chat_ids = [12345678]` and posts to the owner DM. The group never sees the agenda.

## Goals

1. Each cron carries its own delivery target (`chat_id` plus optional `thread_id`).
2. Targets are validated against the live `allowlist.yaml` at creation, update, and delivery time.
3. The legacy `notify_chat_ids` variable disappears from `cron_delivery` and `oauth_callback` — single source of truth.
4. Existing crons with no target fail loud (WARN log + skip) rather than silently misroute.
5. Operator can fix targets via `cron_update` MCP tool or by recreating the cron.
6. Doctor surfaces crons without valid targets.

## Non-Goals

- Per-run dynamic targeting. The target is a property of the cron, not the run.
- Multi-cast (one cron → multiple chats). Single target is enough for current use cases; YAGNI.
- Migration heuristics that try to guess where existing crons should deliver. We have no reliable signal.
- Reworking `agent.yaml::allowed_chat_ids` semantics. The migration to `allowlist.yaml` already happened; this design just stops the cron path from reading the legacy field.

## Design

### Data model

**Schema (`cron_specs`), migration v17:**

```sql
ALTER TABLE cron_specs ADD COLUMN target_chat_id   INTEGER;
ALTER TABLE cron_specs ADD COLUMN target_thread_id INTEGER;
```

Both columns are nullable. `target_chat_id` is nullable only for back-compat with rows that existed before this migration; new rows inserted by `cron_create` always populate it (validated at the MCP layer). `target_thread_id` is always nullable — most chats have no topic.

No automatic back-fill in the migration. Existing rows keep `NULL` and trigger the WARN path described under "Delivery."

The migration is idempotent: it uses `pragma_table_info` to check column presence first, matching the v13 pattern in `crates/rightclaw/src/memory/migrations.rs`.

**`cron_runs.delivery_status`** gains two new values:

- `'no_target'` — cron's `target_chat_id` is `NULL` at delivery time
- `'denied'` — `target_chat_id` is no longer in `allowlist.yaml` at delivery time

Existing values (`pending`, `silent`, `delivered`, `superseded`, `failed`) are unchanged.

### YAML input to the agent

`crates/bot/src/telegram/attachments.rs::format_cc_input` always emits a `chat:` block, regardless of DM or group. Today, `ChatContext::Private` emits no attribution; we change it to emit `chat: { id: <i64> }`. The Group variant continues to emit `id`, `title`, and `topic_id` as before.

This is the only YAML format change. It guarantees the agent always sees `chat.id` and can pass it to `cron_create` without conditional logic.

### MCP tools

**`cron_create`** (`crates/rightclaw-cli/src/right_backend.rs`):

`CronCreateParams` gains:

```rust
pub target_chat_id: i64,             // required
pub target_thread_id: Option<i32>,   // optional
```

Validation in `call_cron_create`:

1. Resolve the agent's `allowlist.yaml` (already loaded into `AllowlistHandle` per agent — exposed to the aggregator via the existing per-agent state path; if the aggregator does not currently hold it, this design adds the lookup).
2. `target_chat_id` must be present in either `users` or `groups` of the allowlist. Otherwise return tool error: `target_chat_id <id> is not in allowlist; use /allow or /allow_all from a trusted account first`.
3. `target_thread_id`, if present, is not validated against any allowlist (Telegram will reject sends to nonexistent threads at delivery time). It is only meaningful for supergroup topic targets.

**`cron_update`** (`crates/rightclaw-cli/src/right_backend.rs`):

`CronUpdateParams` gains two new optional fields. Standard partial-update semantics: omit field → leave as is; provide `target_chat_id` → validate (same rule as create) and overwrite.

For `target_thread_id`, distinguishing "not specified" from "explicitly clear to NULL" requires a double-`Option` in Rust (`Option<Option<i32>>`) with `#[serde(default, deserialize_with = "deserialize_some")]` so missing fields stay `None` and explicit JSON `null` becomes `Some(None)`. JSON Schema exposes the field as `{ "type": ["integer", "null"] }`. This is a known serde idiom — see `serde_with::rust::double_option` for the helper crate, or roll a small `deserialize_some` locally.

`cron_spec::update_spec_partial` extends with two new params and the same allowlist check on `target_chat_id` when present.

**`cron_list`** (`crates/rightclaw-cli/src/right_backend.rs` + `crates/rightclaw/src/cron_spec.rs::list_specs`):

Each row in the output adds `target_chat_id` and `target_thread_id`. Existing fields unchanged.

### Delivery loop

`crates/bot/src/cron_delivery.rs`:

**Signature changes** — drop `notify_chat_ids: Vec<i64>` from both `run_delivery_loop` and `deliver_through_session`. The loop reads target from each row instead.

**`fetch_pending` and `deduplicate_job`** — add `target_chat_id` and `target_thread_id` to the SELECT and to `PendingCronResult`.

**Per-pending decision matrix:**

| `target_chat_id` | In allowlist? | Action |
|---|---|---|
| `NULL` | n/a | log WARN with `cron_update` hint; `mark_delivery_outcome(.., "no_target")` |
| set | no | log WARN; `mark_delivery_outcome(.., "denied")` |
| set | yes | proceed to `deliver_through_session` with the single (chat_id, thread_id) target |

`mark_delivery_outcome` already writes both `delivery_status` and `delivered_at`, so `'no_target'` and `'denied'` rows do not get retried.

**Single-target send** — `deliver_through_session` takes `target_chat_id: i64, target_thread_id: Option<i32>` instead of a slice. The two `for &cid in notify_chat_ids` loops in the function (text send and attachment send) collapse to a single send each. `message_thread_id` is set on `send_message` when `target_thread_id.is_some()`, mirroring the worker's existing pattern (`telegram/worker.rs:567`).

**Session lookup** — currently `notify_chat_ids[0]` is used for `get_active_session`. After the change, `target_chat_id` is used directly. Combined with `target_thread_id` (or `0` when `None`), this matches the existing `(chat_id, effective_thread_id)` session key.

### Allowlist propagation to the aggregator

The MCP aggregator (`right-mcp-server`) runs as a separate process from the bot and does not currently load `allowlist.yaml`. `cron_create` / `cron_update` validation needs read access.

Approach: on each `cron_create` / `cron_update` call, the aggregator reads `~/.rightclaw/agents/<name>/allowlist.yaml` directly via `rightclaw::agent::allowlist::read_file()`. Reads are infrequent (cron writes are rare) and the file is small, so no in-memory cache or watcher is needed. The bot is the sole writer; the aggregator is read-only — file lock contention is irrelevant for read.

Alternatives considered and rejected:
- **In-memory cache + notify watcher in aggregator** — extra moving parts, no measurable win for a tool called a few times per day.
- **Internal API call from aggregator to bot** (`is_chat_allowed` over the existing Unix socket) — couples aggregator to bot uptime and adds a network hop with no benefit.

### OAuth callback

`crates/bot/src/telegram/oauth_callback.rs`:

`OAuthCallbackState` drops `notify_chat_ids: Vec<i64>` and gains `allowlist: AllowlistHandle`. Both notification call sites (`:228` "OAuth completed" and `:238` "OAuth failed") iterate `allowlist.read().users().map(|u| u.id)` — DM owners only, no groups (OAuth is an operator event).

`crates/bot/src/lib.rs:416-431` — the `notify_chat_ids` and `notify_bot` locals fold into the `OAuthCallbackState` construction with the existing `allowlist` handle.

### Doctor

`crates/rightclaw/src/doctor.rs` gains a check `cron_targets`:

For each row in `cron_specs`:
- `target_chat_id IS NULL` → WARN: `cron '{job_name}' has no target_chat_id; use cron_update to set one or recreate the cron in the desired chat`
- `target_chat_id` set but not in `allowlist.yaml` → WARN: `cron '{job_name}' targets chat {id} which is no longer in allowlist`

Healthy rows produce no output. The check needs read access to both `data.db` and `allowlist.yaml` for the agent under inspection — same paths the doctor already uses for other checks.

### Agent-facing instructions

`templates/right/prompt/OPERATING_INSTRUCTIONS.md` and `skills/rightcron/SKILL.md`:

Add a paragraph to the `cron_create` and `cron_update` documentation:

> Always pass `target_chat_id` equal to the `chat.id` you see in the incoming message YAML, unless the user explicitly asks for a different chat. If you are creating a cron from a group message, the target is the group; if from a DM, the target is the DM. Pass `target_thread_id` only when the message arrived inside a supergroup topic and the user wants the cron to reply to that topic specifically.

This is normative — it is the contract the agent must follow. The MCP tool's allowlist validation is the safety net, not a substitute for the instruction.

## Backward Compatibility

Existing cron rows have `target_chat_id IS NULL`. They will not deliver after the migration; instead, each delivery attempt logs a WARN and marks the row `'no_target'`. The operator must run `cron_update` (or recreate the cron) to set a target.

For the `him` agent specifically (one cron, `agenda`, intended for the `aibots` group):

```
cron_update target_chat_id=-1001234567890 job_name=agenda
```

This is documented in the migration release notes and surfaced by the doctor check.

The legacy `agent.yaml::allowed_chat_ids` field is unchanged — the existing `2026-04-16-group-chat-design` migration already handles it. This design just stops cron and OAuth from reading it.

## Testing

**Unit:**
- `cron_spec::create_spec_v2` and `update_spec_partial`: target validation against a fake allowlist (in allowlist → ok; not in allowlist → error; nullable thread_id passes through).
- `migrations.rs`: v17 migration adds columns, leaves existing rows with `NULL`, is idempotent on re-run.
- `doctor.rs::check_cron_targets`: NULL row → WARN; valid row → silent; row with chat removed from allowlist → WARN.
- `cron_delivery::format_cron_yaml` unchanged; `fetch_pending` returns target columns.
- `oauth_callback`: notification iterates allowlist users only.

**Integration:**
- Cron created from group message → delivered to that group, not DM.
- Cron created from DM → delivered to that DM, not group.
- Cron with target removed from allowlist after creation → `'denied'` status, no send.
- Cron with `target_chat_id IS NULL` (simulated existing row) → `'no_target'` status, no send.
- `cron_update` changes target → next run delivers to the new chat.

## Implementation Order

Plan-level concern — left to the implementation plan. The natural sequence is: schema migration → YAML format change for DM `chat.id` → MCP tool validation + signatures → delivery loop rewrite → OAuth callback cutover → doctor check → agent-facing docs → tests at each step.

## Sources

- `crates/bot/src/lib.rs:416, 638` — current `notify_chat_ids` wiring.
- `crates/bot/src/cron_delivery.rs` — delivery loop, dedup, and send.
- `crates/bot/src/telegram/oauth_callback.rs:228, 238` — OAuth notifications.
- `crates/bot/src/telegram/attachments.rs:398-408` — `ChatContext` and YAML attribution.
- `crates/rightclaw-cli/src/right_backend.rs:154` — `cron_create` MCP tool.
- `crates/rightclaw/src/memory/sql/v6_cron_specs.sql` — current schema.
- `crates/rightclaw/src/memory/migrations.rs` — v13 idempotent-add-column pattern.
- `docs/superpowers/specs/2026-04-16-group-chat-design.md` — `allowlist.yaml` source of truth.
