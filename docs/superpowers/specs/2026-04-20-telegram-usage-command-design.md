# Telegram `/usage` Command — Design

**Date:** 2026-04-20
**Status:** Draft
**Scope:** New Telegram bot command that summarizes CC token/cost usage for the whole agent (all interactive chats + cron jobs) across fixed time windows.

## Goal

Give the agent owner a single Telegram command that answers: "how much has this agent been costing me, and where is the cost going?"

Output in one message covers:
- Today, last 7 days, last 30 days, all time (UTC windows).
- Interactive vs Cron split per window.
- Full token breakdown (input / output / cache_creation / cache_read).
- Server tool use counts (web_search, web_fetch).
- Per-model cost breakdown (e.g. claude-sonnet-4-6 vs claude-haiku-4-5).

## Non-Goals

- Per-chat / per-user attribution (we do not store Telegram user_id on sessions).
- Custom date ranges (`/usage 2026-04-01 2026-04-20`) — deferred.
- Backfill from existing `~/.rightclaw/logs/streams/*.ndjson` files — deferred; fresh start on migration.
- Bootstrap CC invocations (one-shot on init, negligible).
- Historical / rolling graphs — text summary only.

## Architecture

One new SQLite table `usage_events` in the per-agent `data.db`. Each row represents **one `claude -p` invocation** (not one logical session) whose `result` event was observed on stdout. Interactive worker and cron engine both write rows. `/usage` handler runs SQL aggregations and formats a Telegram HTML message.

### Modules

- `crates/rightclaw/src/memory/sql/v15_usage_events.sql` — schema.
- `crates/rightclaw/src/memory/migrations.rs` — register migration v15 as `M::up(V15_SCHEMA)` after the existing `M::up(V14_SCHEMA)` entry. Pure SQL with `CREATE TABLE IF NOT EXISTS` + `CREATE INDEX IF NOT EXISTS` — no Rust hook needed.
- `crates/rightclaw/src/usage/mod.rs` — public types: `UsageBreakdown`, `WindowSummary`, `ModelTotals`.
- `crates/rightclaw/src/usage/insert.rs` — `insert_interactive`, `insert_cron`.
- `crates/rightclaw/src/usage/aggregate.rs` — `aggregate(conn, since, source) -> WindowSummary`.
- `crates/bot/src/telegram/stream.rs` — extend with `parse_usage_full(result_json) -> Option<UsageBreakdown>`. Keep existing `parse_usage` (small `StreamUsage` for thinking message) unchanged.
- `crates/bot/src/telegram/worker.rs` — call `insert_interactive` in the `StreamEvent::Result(json)` branch.
- `crates/bot/src/cron.rs` — call `insert_cron` at the same point in cron stream processing. Cron delivery invocations (haiku) get `source='cron'`, `job_name='<job>-delivery'`.
- `crates/bot/src/telegram/dispatch.rs` — add `BotCommand::Usage` enum variant.
- `crates/bot/src/telegram/handler.rs` — `handle_usage(bot, msg, agent_dir)`.

## Schema (Migration v15)

```sql
CREATE TABLE IF NOT EXISTS usage_events (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                     TEXT    NOT NULL,      -- ISO8601 UTC, moment result event received
    source                 TEXT    NOT NULL,      -- 'interactive' | 'cron'
    chat_id                INTEGER,               -- NULL for cron
    thread_id              INTEGER,               -- 0 if none, NULL for cron
    job_name               TEXT,                  -- NULL for interactive
    session_uuid           TEXT    NOT NULL,      -- CC session id from result.session_id
    total_cost_usd         REAL    NOT NULL,
    num_turns              INTEGER NOT NULL,
    input_tokens           INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens      INTEGER NOT NULL DEFAULT 0,
    web_search_requests    INTEGER NOT NULL DEFAULT 0,
    web_fetch_requests     INTEGER NOT NULL DEFAULT 0,
    model_usage_json       TEXT    NOT NULL       -- raw `modelUsage` sub-object as JSON string
);
CREATE INDEX IF NOT EXISTS idx_usage_events_ts ON usage_events (ts);
CREATE INDEX IF NOT EXISTS idx_usage_events_source_ts ON usage_events (source, ts);
```

Note: `session_uuid` is **not unique** — a single logical CC session that is resumed over N turns produces N `result` events, hence N rows. `COUNT(*)` in aggregations therefore counts invocations, labelled "sessions" in output for user-friendliness (exact wording acceptable since the count is still meaningful — each row is one "claude -p run").

## Types

```rust
// In crates/rightclaw/src/usage/mod.rs

pub struct UsageBreakdown {
    pub session_uuid: String,
    pub total_cost_usd: f64,
    pub num_turns: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub model_usage_json: String,        // raw JSON from result.modelUsage
}

pub struct ModelTotals {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

pub struct WindowSummary {
    pub source: String,                          // 'interactive' | 'cron'
    pub cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,                        // COUNT(*)
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: BTreeMap<String, ModelTotals>,
}
```

## Write Path

### Parsing (`stream.rs`)

Extend with:

```rust
pub fn parse_usage_full(result_json: &str) -> Option<UsageBreakdown>
```

Returns `None` if `total_cost_usd` or `num_turns` cannot be extracted (malformed lines do not produce zero-filled rows). Extracts:

- `total_cost_usd`, `num_turns`, `session_id`.
- `usage.input_tokens`, `usage.output_tokens`, `usage.cache_creation_input_tokens`, `usage.cache_read_input_tokens`.
- `usage.server_tool_use.web_search_requests`, `usage.server_tool_use.web_fetch_requests`.
- `modelUsage` — stored as serialized JSON string (per-model reduction happens at read time).

### Interactive (`worker.rs`)

In the stream loop, at the branch `StreamEvent::Result(json)`:

```rust
if let Some(breakdown) = stream::parse_usage_full(json) {
    if let Err(e) = usage::insert_interactive(
        &conn, &breakdown, chat_id, eff_thread_id,
    ) {
        tracing::warn!(?chat_id, "usage insert failed: {e:#}");
    }
}
```

Best-effort: failure logs a warning and the turn continues. Telemetry must not block the user path.

Assumption: a `Connection` handle to `data.db` is already in scope at the `StreamEvent::Result` branch in `worker.rs` (used elsewhere there for `deactivate_current`). If the handle is not directly reachable, the implementation plan must add it via the existing `WorkerContext`.

### Cron (`cron.rs`, `cron_delivery.rs`)

Same hook at the `result` event site. `source='cron'`, `chat_id=NULL`, `thread_id=NULL`, `job_name=<spec.name>`. Delivery sessions (haiku) use `job_name='<job>-delivery'` so their cost is attributed but distinguishable.

### Not Written

- Bootstrap CC runs (init-time, one-shot).
- Any CC invocations triggered by internal tooling outside the worker + cron paths. If such paths exist (e.g. `base:codex`-style) they are currently out of scope; they can opt in later with `source='meta'`.

## Read Path

### Command wiring

`dispatch.rs`:
```rust
#[command(description = "Show usage summary")]
Usage,
```

`handler.rs`:
```rust
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    agent_dir: Arc<AgentDir>,
) -> Result<(), RequestError>
```

Opens `data.db` read-only (or reuses bot's handle), calls `aggregate` 8 times (4 windows × 2 sources), renders the HTML message, sends it.

### Aggregation

```rust
pub fn aggregate(
    conn: &Connection,
    since: Option<DateTime<Utc>>,
    source: &str,
) -> rusqlite::Result<WindowSummary>
```

One SQL query for numeric sums:
```sql
SELECT
    COALESCE(SUM(total_cost_usd), 0.0)        AS cost,
    COALESCE(SUM(num_turns), 0)               AS turns,
    COUNT(*)                                  AS invocations,
    COALESCE(SUM(input_tokens), 0)            AS input,
    COALESCE(SUM(output_tokens), 0)           AS output,
    COALESCE(SUM(cache_creation_tokens), 0)   AS cache_c,
    COALESCE(SUM(cache_read_tokens), 0)       AS cache_r,
    COALESCE(SUM(web_search_requests), 0)     AS web_s,
    COALESCE(SUM(web_fetch_requests), 0)      AS web_f
FROM usage_events
WHERE source = ?1 AND (?2 IS NULL OR ts >= ?2)
```

Second query for per-model: `SELECT model_usage_json FROM ... WHERE ...`. Parse each row's JSON (`{"<model>": {"costUSD":..., "inputTokens":..., ...}}`) and reduce into `BTreeMap<String, ModelTotals>` in Rust. Row counts are small (hundreds to low thousands per month), so Rust-side reduce is fine.

### Windows

All times UTC.
- Today: `ts >= <today 00:00 UTC>`.
- 7 days: `ts >= now - 7d`.
- 30 days: `ts >= now - 30d`.
- All time: `since = None`.

Rationale: UTC is consistent with how CC and our logs are already timestamped. Agent-local timezone is not stored anywhere; introducing it is out of scope. Header of the output explicitly says `(UTC)`.

### Output Format (Telegram HTML)

```
📊 Usage Summary (UTC)

━━ Today ━━
💬 Interactive: $1.23 · 45 turns · 12 invocations
   Tokens: in 1.2k, out 850, cache_c 45k, cache_r 890k
   • claude-sonnet-4-6 — $1.20
   • claude-haiku-4-5  — $0.03
⏰ Cron: $0.15 · 8 turns · 3 runs
   • claude-sonnet-4-6 — $0.15
🔎 Web tools: 2 search, 5 fetch

━━ Last 7 days ━━
...

━━ Last 30 days ━━
...

━━ All time ━━
...

Total all time: $12.45
```

Formatting rules:
- Numbers ≥ 1000 → `k` / `M` suffix with one decimal (`1.2k`, `3.4M`).
- Costs → `$X.XX` (two decimals). Below `$0.01` show `<$0.01`.
- `WindowSummary.invocations` is rendered as "invocations" for `source=interactive` and "runs" for `source=cron` (user-facing wording).
- Empty window: `(no activity)` under the header instead of the sub-lines.
- Empty DB: single line `No usage recorded yet.`
- Web tools line omitted if both counters are 0 in this window.
- Interactive or Cron sub-section omitted entirely when its invocations=0 in the window. If both are 0 → treat as empty window.
- All dynamic strings HTML-escaped (consistent with `super::markdown::html_escape`).

Message size: 4 windows × ~600 chars ≈ 2.5 KB, well within the 4096 char Telegram limit.

## Error Handling

| Failure | Behavior |
|---|---|
| INSERT fails (write path) | `tracing::warn!`, turn continues. |
| `parse_usage_full` returns `None` (no `total_cost_usd` / `num_turns`) | Skip INSERT entirely. Better no row than zero-filled row that skews averages. |
| DB open fails in handler | Reply `Failed to read usage: <err>`. |
| SQL fails in handler | Same format. |
| Empty DB | `No usage recorded yet.` |
| Migration rerun | `CREATE TABLE IF NOT EXISTS` + `CREATE INDEX IF NOT EXISTS` are idempotent; `rusqlite_migration` skips versions already applied. |

## Testing

### Unit tests

- `stream::parse_usage_full` — happy path using a real `result` line from an existing NDJSON fixture; missing `total_cost_usd` → `None`; missing `modelUsage` → empty JSON string `{}`; malformed JSON → `None`.
- `usage::aggregate` — fixture with 10 rows spanning multiple days and both sources; assert sums, invocation counts, per-model reduction across two models, empty window returns zeroed `WindowSummary`, empty table returns zeroed summary.
- `usage::insert_interactive` / `insert_cron` — round-trip: insert, read back, compare.
- Migration v14 → v15 on empty DB and DB with prior data; verify indexes present via `pragma_index_list`.

### Manual verification after merge

1. Fresh agent → `/usage` → `No usage recorded yet.`
2. A few interactive messages → `/usage` shows them under Today → Interactive.
3. Run one cron job → `/usage` shows it under Today → Cron.
4. Confirm per-model lines appear when agent uses a model override.
5. Confirm `(no activity)` behavior on an empty window (e.g. a brand-new 30-day window by setting system clock or waiting).

### Not tested automatically

End-to-end through teloxide. The handler is thin (DB → format → send). Aggregation and formatting logic are covered by unit tests; handler-level smoke is manual.

## Open Questions

None blocking. Potential follow-ups (not this spec):
- Retention / pruning of `usage_events` (unbounded growth — ~1 KB/row, 1000 rows/month ≈ 1 MB/month, not urgent).
- Custom date range arg.
- Per-chat breakdown once user_id is tracked on sessions.
- Backfill from historical NDJSON files.
