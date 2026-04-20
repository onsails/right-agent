# /usage Command Redesign Design

**Status:** Draft
**Date:** 2026-04-20
**Depends on:** `2026-04-20-telegram-usage-command-design.md` (v15 schema, `usage_events` table, initial `/usage` handler)

## Goal

Rework the `/usage` Telegram command output to be human-readable and actionable:

1. Replace cryptic `cache_c` / `cache_r` raw-token lines with **prompt-cache effectiveness** (hit rate + dollar savings).
2. Distinguish **subscription-covered** invocations from **API-billed** invocations via `apiKeySource` captured from the CC `system/init` event.
3. Expose raw tokens only via `/usage detail` (opt-in).
4. Fix the slash-command autocomplete menu so `/usage` appears alongside other commands.

## Non-Goals

- No tracking of actual Anthropic invoice or true subscription overage (requires billing API we don't have).
- No configurable plan-budget heuristic ("am I close to my weekly cap?") — deferred.
- No per-invocation audit log view — deferred to a future `/usage log` command.
- No backfill of `apiKeySource` on existing rows beyond the safe default `'none'` (current RightClaw deployments use setup-token = subscription).

## Architecture Overview

The existing `/usage` pipeline is:

```
claude -p stream-json ─► parse_usage_full ─► insert_interactive/insert_cron
                                                        │
                                                        ▼
                                             usage_events (SQLite)
                                                        │
                                                        ▼
                       /usage ─► aggregate ─► format_summary_message ─► Telegram HTML
```

Changes ride on this pipeline:

- **Capture new signal**: `apiKeySource` from the *first* stream event (`type=system, subtype=init`), stashed per-worker and passed into the insert. Requires a new parse helper.
- **Schema v16**: one additive column, idempotent migration.
- **Aggregation**: two new sum columns (subscription vs API split).
- **Rendering**: full rewrite of `format.rs` — new section layout, cache line, billing labels, subscription-covered footnote, two modes (default / detail).
- **Pricing table**: new `usage::pricing` module with hardcoded per-model rates for cache-savings math.
- **Handler**: `Usage(String)` variant parses optional `detail` arg; flag threaded into renderer.
- **Menu fix**: proactive `delete_my_commands` pass for language-scoped overrides on bot startup.

## Data Model

### v16 Migration

New file: `crates/rightclaw/src/memory/sql/v16_usage_api_key_source.sql`

```sql
-- Add api_key_source to usage_events, idempotent via pragma_table_info check.
-- Column default 'none' backfills legacy rows as subscription invocations
-- (RightClaw's setup-token auth flow sets apiKeySource='none' in CC).
-- (Applied via M::up_with_hook in migrations.rs — see notes below.)
ALTER TABLE usage_events ADD COLUMN api_key_source TEXT NOT NULL DEFAULT 'none';
```

Registered in `migrations.rs` after the existing v15 entry:

```rust
const V16_SCHEMA: &str = include_str!("sql/v16_usage_api_key_source.sql");
// ...
M::up_with_hook(V16_SCHEMA, |tx: &Transaction| {
    // Skip ALTER if column already exists (idempotency guard).
    let exists: bool = tx.query_row(
        "SELECT 1 FROM pragma_table_info('usage_events') WHERE name='api_key_source'",
        [],
        |_| Ok(true),
    ).unwrap_or(false);
    if exists { return Ok(()); }
    tx.execute_batch(V16_SCHEMA)?;
    Ok(())
}),
```

### Rust Types

`crates/rightclaw/src/usage/mod.rs`:

```rust
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
    pub model_usage_json: String,
    pub api_key_source: String,   // NEW — 'none' means subscription
}

pub struct WindowSummary {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,   // NEW
    pub api_cost_usd: f64,            // NEW
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: BTreeMap<String, ModelTotals>,
}
```

`ModelTotals` unchanged.

### Aggregate SQL

In `aggregate.rs` the main aggregate query adds two sums:

```sql
SELECT
    COALESCE(SUM(total_cost_usd), 0) AS cost,
    COALESCE(SUM(CASE WHEN api_key_source = 'none' THEN total_cost_usd ELSE 0 END), 0) AS sub_cost,
    COALESCE(SUM(CASE WHEN api_key_source != 'none' THEN total_cost_usd ELSE 0 END), 0) AS api_cost,
    COALESCE(SUM(num_turns), 0) AS turns,
    COUNT(*) AS invocations,
    ...
FROM usage_events
WHERE source = ?1 AND (?2 IS NULL OR ts >= ?2)
```

`aggregate_per_model` unchanged (per-model split operates on `model_usage_json` regardless of billing mode).

## Source Capture

### New parse helper

`crates/bot/src/telegram/stream.rs`:

```rust
/// Parse `apiKeySource` from the CC `system/init` NDJSON event.
/// Returns `None` if the input is not JSON or lacks the field.
pub fn parse_api_key_source(init_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(init_json).ok()?;
    if v.get("type")?.as_str()? != "system" { return None; }
    if v.get("subtype")?.as_str()? != "init" { return None; }
    v.get("apiKeySource")?.as_str().map(|s| s.to_string())
}
```

### Worker capture

`crates/bot/src/telegram/worker.rs`, inside the stream loop:

```rust
// Before the loop:
let mut api_key_source: Option<String> = None;

// In the line-handling branch, BEFORE parse_stream_event:
if api_key_source.is_none()
    && let Some(src) = super::stream::parse_api_key_source(&line)
{
    api_key_source = Some(src);
}

// When inserting (on Result event), populate:
let mut breakdown = super::stream::parse_usage_full(json)?;
breakdown.api_key_source = api_key_source.clone().unwrap_or_else(|| "none".into());
rightclaw::usage::insert::insert_interactive(&conn, &breakdown, chat_id, eff_thread_id)?;
```

`parse_usage_full` no longer sets `api_key_source` itself — the worker owns the field because it has access to the init event the result event lacks. `UsageBreakdown::api_key_source` is populated at the insertion site.

### Cron capture

`crates/bot/src/cron.rs`, after `collected_lines` is fully populated:

```rust
// Find init event (first matching line; fallback "none").
let api_key_source = collected_lines
    .iter()
    .find_map(|l| crate::telegram::stream::parse_api_key_source(l))
    .unwrap_or_else(|| "none".into());

// Existing: find last result line, parse breakdown, then:
breakdown.api_key_source = api_key_source;
rightclaw::usage::insert::insert_cron(&conn, &breakdown, job_name)?;
```

### cron_delivery (unchanged)

Delivery uses non-streaming JSON output — no init event in the payload. Keep existing TODO comment.

## Pricing + Cache-Savings Math

### New module `crates/rightclaw/src/usage/pricing.rs`

```rust
#[derive(Clone, Copy)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Hardcoded table. Update when Anthropic changes published rates.
/// Unknown model → None → caller omits dollar figure from output.
pub fn lookup(model: &str) -> Option<ModelPricing> {
    match model {
        "claude-sonnet-4-6" => Some(ModelPricing {
            input_per_mtok: 3.0, output_per_mtok: 15.0,
        }),
        "claude-opus-4-7" => Some(ModelPricing {
            input_per_mtok: 15.0, output_per_mtok: 75.0,
        }),
        m if m.starts_with("claude-haiku-4-5") => Some(ModelPricing {
            input_per_mtok: 0.80, output_per_mtok: 4.0,
        }),
        _ => None,
    }
}
```

Haiku match uses `starts_with` to handle dated variants like `claude-haiku-4-5-20251001`.

### Cache effectiveness

Per model (in `format.rs`):

```rust
fn cache_metrics(model: &str, t: &ModelTotals) -> (f64, Option<f64>) {
    let total_input = t.input_tokens + t.cache_creation_tokens + t.cache_read_tokens;
    let hit_rate = if total_input == 0 {
        0.0
    } else {
        t.cache_read_tokens as f64 / total_input as f64
    };
    let savings = pricing::lookup(model).map(|p| {
        // Cache reads cost 10% of regular input → saved 90% of full input cost.
        t.cache_read_tokens as f64 * p.input_per_mtok * 0.9 / 1_000_000.0
    });
    (hit_rate, savings)
}
```

Aggregated per window (across all models in that window's `per_model`):
- **Hit rate**: token-weighted average — `sum(cache_read) / sum(total_input)` across models.
- **Savings**: sum of per-model savings. If any model's pricing is unknown, render `~$X` as `~$X (partial)` OR drop the dollar clause entirely if *no* model is priced. Simpler: omit dollar clause only when the sum is zero (i.e. zero priced models). Still show the hit-rate portion.

Line rendering:

```
Cache: 71% hit rate, saved ~$2.80
Cache: 71% hit rate                    ← unknown-model fallback (no $)
                                       ← line omitted when cache_read_tokens == 0
```

## Rendering

### `format_summary_message` signature

```rust
pub fn format_summary_message(w: &AllWindows, detail: bool) -> String;
```

### Layout per window

```
━━ <Window Label> ━━
💬 Interactive<BillingTag>: $X retail · N turns · M sessions
  <Per-model cost lines>
  Cache: H% hit rate, saved ~$C.CC     ← omitted when no cache
  Tokens: N new in, N out, N cache-created, N cache-read    ← detail mode only
⏰ Cron<BillingTag>: ...
  (same shape)
🔎 Web: N searches, M fetches          ← omitted when all zero
<FooterNote>                            ← per-window subscription footnote (see below)
```

`<BillingTag>` decision (per source, per window):
- Subscription cost > 0 AND API cost == 0 → `""` (no tag)
- Subscription cost == 0 AND API cost > 0 → `" (API-billed)"`
- Both > 0 → `" (Mixed)"` — per-model and cache lines are rendered combined; billing split is shown in the footer only (see below).

Rationale: per-model cost and cache-effectiveness are orthogonal to billing mode. A single Sonnet-4.6 block that happens to contain both subscription and API invocations still has one valid hit-rate and one total cost. Splitting it per mode adds SQL complexity without adding insight — the meaningful split is the dollar total, which the footer already carries.

### Footer per window (below blocks)

- If `api_cost_usd == 0.0`: `Subscription covers this (Claude subscription via setup-token)`
- If `subscription_cost_usd == 0.0`: `Billed via API key`
- If both non-zero: `Subscription: $X.XX · API-billed: $Y.YY`
- If window empty: `(no activity)` (unchanged)

### Footer total (bottom of message)

```
Total retail: $A.AA · subscription: $B.BB · API-billed: $C.CC
```

Only show the breakdown if both subscription and API > 0; else show `Total retail: $A.AA` only, matching the current format.

### Detail mode

`detail=true` adds under each source block:

```
  Tokens: N new in, N out, N cache-created, N cache-read
```

Formatter reuses `format_count` helper (unchanged).

## Handler + Menu

### `/usage detail` argument

`crates/bot/src/telegram/dispatch.rs`:

```rust
#[command(description = "Show usage summary (add 'detail' for raw tokens)")]
Usage(String),   // was: Usage
```

Handler wiring:

```rust
.branch(dptree::case![BotCommand::Usage(arg)].endpoint(handle_usage))
```

`crates/bot/src/telegram/handler.rs`:

```rust
pub async fn handle_usage(
    bot: BotType,
    msg: Message,
    arg: String,
    agent_dir: Arc<AgentDir>,
) -> ResponseResult<()> {
    if !is_private_chat(&msg.chat.kind) {
        tracing::debug!(cmd = "usage", "ignoring command in group chat (DM-only)");
        return Ok(());
    }
    let detail = matches!(arg.trim().to_ascii_lowercase().as_str(), "detail" | "d");
    let text = match build_usage_summary(&agent_dir.0, detail).await {
        Ok(t) => t,
        Err(e) => format!("Failed to read usage: {e:#}"),
    };
    let eff_thread_id = effective_thread_id(&msg);
    send_html_reply(&bot, msg.chat.id, eff_thread_id, &text).await?;
    Ok(())
}

async fn build_usage_summary(agent_dir: &Path, detail: bool) -> Result<String, miette::Report> {
    // ... existing window aggregation ...
    Ok(format_summary_message(&windows, detail))
}
```

Unknown args fall through to default (no detail). No error.

### Menu fix

`crates/bot/src/telegram/dispatch.rs` — add before the existing `set_my_commands` loop:

```rust
// Proactively clear any language-scoped command lists from prior deployments.
// Telegram resolution order: scope+language wins over scope-only, so stale
// language-scoped entries mask our fresh set. Best-effort, ignore errors.
for scope in [
    teloxide::types::BotCommandScope::Default,
    teloxide::types::BotCommandScope::AllPrivateChats,
    teloxide::types::BotCommandScope::AllGroupChats,
] {
    for lang in ["en", "ru"] {
        let _ = bot
            .delete_my_commands()
            .scope(scope.clone())
            .language_code(lang.to_string())
            .await;
    }
}
// ... existing set_my_commands loop (unchanged) ...
```

Note in operator docs: after upgrade, if `/usage` still doesn't appear in the slash menu, restart the Telegram client once — client-side command cache is out of our control.

## Error Handling

- Migration failure: propagated up via `rusqlite_migration` — bot will fail to start, logged loudly. Correct behaviour (same as all other migrations).
- `parse_api_key_source` returns None → worker stores `"none"` default. Non-fatal, logged at `debug` level.
- Insert failure post-migration: already handled (`tracing::warn!` + continue — best-effort telemetry pattern established in v15).
- Pricing lookup miss: cache line renders without dollar clause. No warning — common for less-used models.
- Aggregate SQL with new `CASE`: runs under the same single-transaction snapshot as before; no consistency regression.
- Mixed-billing second-aggregate call failure: treated same as main aggregate failure (error propagated to handler, shown as `Failed to read usage: <chain>`).

## Testing

### Unit tests

**`usage/pricing.rs`:**
- `lookup("claude-sonnet-4-6")` → Some with expected rates.
- `lookup("claude-opus-4-7")` → Some.
- `lookup("claude-haiku-4-5-20251001")` → Some (starts_with matches).
- `lookup("unknown-model")` → None.

**`usage/format.rs`:**
- Default mode: no `Tokens:` line.
- Detail mode: `Tokens: N new in, N out, N cache-created, N cache-read`.
- Subscription-only window: footer `Subscription covers this (Claude subscription via setup-token)`.
- API-only window: block label `(API-billed)`, footer `Billed via API key`.
- Mixed-billing window: footer `Subscription: $X · API-billed: $Y`.
- Cache line with known model: `71% hit rate, saved ~$2.80`.
- Cache line with unknown model: `71% hit rate` (no dollar).
- Cache-read == 0: cache line omitted entirely.
- Total footer: splits when both modes present, single-line otherwise.
- HTML escape unchanged (existing `foo<script>` test).

**`usage/aggregate.rs`:**
- Subscription-only rows → `subscription_cost_usd == cost_usd`, `api_cost_usd == 0`.
- API-only rows → inverse.
- Mixed rows → both non-zero, sum matches total.

**`usage/insert.rs`:**
- Insert with `api_key_source="none"` round-trips.
- Insert with `api_key_source="ANTHROPIC_API_KEY"` round-trips.

**`telegram/stream.rs`:**
- `parse_api_key_source` on real init-event fixture → Some("none").
- `parse_api_key_source` on result event → None (wrong type).
- `parse_api_key_source` on malformed JSON → None.
- `parse_api_key_source` on init event without the field → None.

### Migration tests

- `memory/mod.rs`: rename `user_version_is_15` → `user_version_is_16`, bump constant.
- `doctor.rs`: same `user_version` constant bump.
- Existing DB with v15 schema → applying v16 adds column, default backfills to `'none'`, idempotent re-run is a no-op.

### Manual verification (post-merge, user-driven)

1. Run on an agent with existing `usage_events` rows.
2. Bot startup: tail logs for v16 migration success.
3. `sqlite3 ~/.rightclaw/agents/<name>/data.db 'PRAGMA table_info(usage_events);'` shows `api_key_source` column.
4. `SELECT api_key_source FROM usage_events` returns `'none'` for all existing rows.
5. Send a message → `/usage` shows new format, no `cache_c`/`cache_r`, cache line present, subscription footnote.
6. `/usage detail` shows raw-tokens line.
7. Trigger a cron job → `/usage` picks up `(⏰ Cron)` block with cache line.
8. Restart Telegram client; slash menu shows `/usage`.
9. (Optional) Override `ANTHROPIC_API_KEY` for one test invocation to verify `api_key_source != 'none'` is captured and rendered with `(API-billed)` label.

## File Map

| Change | File | Kind |
|---|---|---|
| v16 SQL | `crates/rightclaw/src/memory/sql/v16_usage_api_key_source.sql` | new |
| Register v16 | `crates/rightclaw/src/memory/migrations.rs` | modify |
| `UsageBreakdown`, `WindowSummary` fields | `crates/rightclaw/src/usage/mod.rs` | modify |
| Pricing table | `crates/rightclaw/src/usage/pricing.rs` | new |
| `insert_{interactive,cron}` include `api_key_source` | `crates/rightclaw/src/usage/insert.rs` | modify |
| Aggregate SQL adds `sub_cost`/`api_cost` sums | `crates/rightclaw/src/usage/aggregate.rs` | modify |
| Rendering (default + detail + billing labels) | `crates/rightclaw/src/usage/format.rs` | rewrite |
| `parse_api_key_source` | `crates/bot/src/telegram/stream.rs` | modify |
| Init-event capture, pass to insert | `crates/bot/src/telegram/worker.rs` | modify |
| Same, for cron | `crates/bot/src/cron.rs` | modify |
| `Usage(String)` variant, handler arg | `crates/bot/src/telegram/dispatch.rs`, `handler.rs` | modify |
| Menu fix: pre-delete language-scoped | `crates/bot/src/telegram/dispatch.rs` | modify |
| `user_version` bump | `crates/rightclaw/src/memory/mod.rs`, `doctor.rs` | modify |

## Open Questions / Risks

- **Pricing drift**: hardcoded table will go stale when Anthropic changes rates. Acceptable — savings figure is an estimate labelled `~$`. Mitigation: add a comment in `pricing.rs` pointing to https://www.anthropic.com/pricing and flag it in agent CHANGELOG when rates change.
- **Mixed-billing rendering**: combining per-model and cache data across both billing modes in a single block is a deliberate simplification. Agents switching auth mode mid-window is rare (setup-token auth is long-lived); when it happens, the user still sees accurate per-model totals and the billing split in the footer.
- **Unknown-model cache savings**: silently drops the dollar clause. User sees just `71% hit rate`. If several new models appear, operator should add entries to `pricing.rs`.
- **Telegram command cache**: post-upgrade, clients may still show the stale menu until restart. Documented as expected, no code fix possible.
