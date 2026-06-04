# Usage Panel Local-Time Ranges Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Usage tab compute and display local-calendar time windows with explicit ranges, plus token legends at both top and bottom.

**Architecture:** The dashboard frontend sends the viewer's browser timezone to the Usage API. The Rust read model resolves that timezone, computes local calendar windows and local daily buckets, converts bounds back to UTC for storage filtering, and returns explicit range metadata. Vue renders the range metadata and adds a top token legend while preserving the existing sticky bottom legend.

**Tech Stack:** Rust edition 2024, `right-dashboard` read model/API types, `bot` Axum dashboard route, `chrono` + `chrono-tz`, Vue 3 `<script setup>`, TypeScript, vitest SSR, pnpm.

---

## Implementation Prerequisite

Before any Rust code edits, load the repo-required `rust-dev:rust-dev` skill. If that skill is unavailable in the implementation session, stop and tell the user before editing Rust.

## File Structure

- Modify `Cargo.toml` - add workspace dependency `chrono-tz = "0.10"` (latest verified as `0.10.4`; project policy uses `x.x` requirements).
- Modify `crates/right-dashboard/Cargo.toml` - depend on workspace `chrono-tz`.
- Modify `crates/right-dashboard/src/api_types.rs` - add effective `timezone` to `UsageOverviewResponse`; add `range_start`, `range_end`, and `range_label` to `UsageWindow`.
- Modify `crates/right-dashboard/src/read_model/usage.rs` - thread timezone-aware bounds through the Usage read model.
- Create `crates/right-dashboard/src/read_model/usage_time.rs` - focused timezone/range helper, keeping `usage.rs` from growing more than necessary.
- Create `crates/right-dashboard/src/read_model/usage_local_time_tests.rs` - focused read-model tests for local calendar windows and fallback behavior.
- Modify `crates/bot/src/telegram/dashboard.rs` - parse `timezone` query parameter for `/api/v1/usage`.
- Modify `crates/right-dashboard/frontend/src/types.ts` - mirror new Usage API fields.
- Modify `crates/right-dashboard/frontend/src/api.ts` - send browser timezone query parameter.
- Create `crates/right-dashboard/frontend/src/api.test.ts` - unit test the timezone query path.
- Create `crates/right-dashboard/frontend/src/views/usageDayRange.ts` - pure selected-day range formatter.
- Create `crates/right-dashboard/frontend/src/views/usageDayRange.test.ts` - unit tests for selected-day range labels.
- Modify `crates/right-dashboard/frontend/src/views/UsageView.vue` - render top legend and window range sublines; pass selected-day range to breakdown.
- Modify `crates/right-dashboard/frontend/src/views/UsageView.test.ts` - update stubs and assert top/bottom legends plus range labels.
- Modify `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue` - make sticky positioning optional so only the bottom legend sticks.
- Modify `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue` - accept and render selected-day range label.
- Modify `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts` - assert range label rendering.
- Read `docs/architecture/modules.md` during implementation. Update it only if the dashboard read-model description is now materially drifted; otherwise leave it unchanged.

## Task 1: Rust Usage API Contract Tests

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`
- Modify: `crates/right-dashboard/src/read_model/usage.rs`
- Create: `crates/right-dashboard/src/read_model/usage_local_time_tests.rs`

- [ ] **Step 1: Add the failing API type fields**

In `crates/right-dashboard/src/api_types.rs`, extend the structs exactly this way:

```rust
// crates/right-dashboard/src/api_types.rs
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub timezone: String,
    pub windows: Vec<UsageWindow>,
    pub selected_window: String,
    pub daily_series: Vec<UsageDailyPoint>,
    pub source_series: Vec<UsageSourceSeries>,
    pub warnings: Vec<DashboardDataWarning>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageWindow {
    pub key: String,
    pub label: String,
    pub range_start: Option<String>,
    pub range_end: String,
    pub range_label: String,
    pub sources: Vec<UsageSourceSummary>,
    pub total_cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: Vec<UsageModelSummary>,
    #[serde(default)]
    pub budget_skip_count: i64,
}
```

Expected compile state: existing constructors now fail until later tasks populate the new fields.

- [ ] **Step 2: Thread the new input field**

In `crates/right-dashboard/src/read_model/usage.rs`, add the field to `UsageOverviewInput`:

```rust
// crates/right-dashboard/src/read_model/usage.rs
pub struct UsageOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub timezone: Option<String>,
}
```

Expected compile state: existing test and handler constructors now fail until later steps set `timezone`.

- [ ] **Step 3: Add focused local-time regression tests**

Append this module include near the bottom of `crates/right-dashboard/src/read_model/usage.rs`, after the existing inline test module:

```rust
// crates/right-dashboard/src/read_model/usage.rs
#[cfg(test)]
#[path = "usage_local_time_tests.rs"]
mod usage_local_time_tests;
```

Create `crates/right-dashboard/src/read_model/usage_local_time_tests.rs`:

```rust
// crates/right-dashboard/src/read_model/usage_local_time_tests.rs
use super::*;
use right_db::{params, open_connection};
use tempfile::tempdir;

async fn insert_usage(conn: &right_db::Connection, ts: &str, source: &str, cost: f64) {
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, NULL, ?3, ?4, 1, 10, 20, 5, 40, 0, 0,
            '{\"sonnet\":{\"costUSD\":0.0,\"inputTokens\":10,\"outputTokens\":20,\"cacheCreationInputTokens\":5,\"cacheReadInputTokens\":40}}',
            'none')",
        params![ts, source, format!("{source}-{ts}"), cost],
    )
    .await
    .unwrap();
}

fn window<'a>(response: &'a crate::api_types::UsageOverviewResponse, key: &str) -> &'a crate::api_types::UsageWindow {
    response.windows.iter().find(|window| window.key == key).unwrap()
}

#[tokio::test]
async fn usage_overview_uses_requested_timezone_for_today() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    insert_usage(&conn, "2026-06-03T19:59:59Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-06-03T20:00:00Z", "interactive", 1.25).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "interactive", 2.50).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "Asia/Dubai");
    let today = window(&response, "today");
    let interactive = today.sources.iter().find(|source| source.source == "interactive").unwrap();
    assert_eq!(interactive.invocations, 2);
    assert!((interactive.cost_usd - 3.75).abs() < 1e-9);
    assert_eq!(today.range_start.as_deref(), Some("2026-06-04T00:00:00+04:00"));
    assert_eq!(today.range_end, "2026-06-04T20:47:36+04:00");
    assert_eq!(today.range_label, "Asia/Dubai · Jun 4 00:00-20:47");
}

#[tokio::test]
async fn usage_overview_uses_local_calendar_windows_not_rolling_hours() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    insert_usage(&conn, "2026-05-29T19:59:59Z", "cron", 9.99).await;
    insert_usage(&conn, "2026-05-29T20:00:00Z", "cron", 1.00).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "cron", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
        },
    )
    .await
    .unwrap();

    let last_7_days = window(&response, "last_7_days");
    let cron = last_7_days.sources.iter().find(|source| source.source == "cron").unwrap();
    assert_eq!(cron.invocations, 2);
    assert!((cron.cost_usd - 3.00).abs() < 1e-9);
    assert_eq!(last_7_days.range_start.as_deref(), Some("2026-05-29T00:00:00+04:00"));
    assert_eq!(last_7_days.range_label, "Asia/Dubai · May 29 00:00-Jun 4 20:47");
}

#[tokio::test]
async fn usage_overview_invalid_timezone_falls_back_to_utc_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-04T01:00:00Z", "interactive", 0.10).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("Not/AZone".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "UTC");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.timezone"
            && warning.kind == "invalid_timezone"
            && warning.message.contains("Not/AZone")
    }));
    assert_eq!(window(&response, "today").range_start.as_deref(), Some("2026-06-04T00:00:00+00:00"));
}

#[tokio::test]
async fn usage_overview_missing_timezone_falls_back_to_utc_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.timezone, "UTC");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.timezone" && warning.kind == "missing_timezone"
    }));
}
```

- [ ] **Step 4: Run the failing Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_local_time_tests
```

Expected: FAIL to compile because `timezone`, `range_start`, `range_end`, and `range_label` are not populated yet, and `chrono_tz` is not available.

- [ ] **Step 5: Keep the red state uncommitted**

Do not commit this non-compiling state. Continue directly to Task 2, then commit the tests and implementation together once the targeted Rust tests pass.

## Task 2: Timezone-Aware Usage Read Model

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/right-dashboard/Cargo.toml`
- Create: `crates/right-dashboard/src/read_model/usage_time.rs`
- Modify: `crates/right-dashboard/src/read_model/usage.rs`
- Modify: existing tests in `crates/right-dashboard/src/read_model/usage.rs`

- [ ] **Step 1: Add the timezone dependency**

In root `Cargo.toml`, add this workspace dependency near `chrono`:

```toml
# Cargo.toml
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.10"
```

In `crates/right-dashboard/Cargo.toml`, add:

```toml
# crates/right-dashboard/Cargo.toml
chrono-tz = { workspace = true }
```

- [ ] **Step 2: Add the timezone helper module**

Create `crates/right-dashboard/src/read_model/usage_time.rs`:

```rust
// crates/right-dashboard/src/read_model/usage_time.rs
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use crate::api_types::DashboardDataWarning;

use super::ReadModelError;

#[derive(Clone)]
pub(crate) struct UsageClock {
    pub timezone: String,
    pub tz: Tz,
    pub now_utc: DateTime<Utc>,
    pub now_local: DateTime<Tz>,
    pub warnings: Vec<DashboardDataWarning>,
}

#[derive(Clone)]
pub(crate) struct UsageWindowRange {
    pub key: &'static str,
    pub label: &'static str,
    pub since_utc: Option<DateTime<Utc>>,
    pub until_utc: DateTime<Utc>,
    pub range_start: Option<String>,
    pub range_end: String,
    pub range_label: String,
}

pub(crate) fn resolve_usage_clock(
    generated_at: &str,
    requested_timezone: Option<&str>,
) -> Result<UsageClock, ReadModelError> {
    let now_utc = DateTime::parse_from_rfc3339(generated_at)?.with_timezone(&Utc);
    let requested = requested_timezone.map(str::trim).filter(|value| !value.is_empty());
    let mut warnings = Vec::new();

    let (tz, timezone) = match requested {
        Some(raw) => match raw.parse::<Tz>() {
            Ok(tz) => (tz, raw.to_owned()),
            Err(_) => {
                warnings.push(DashboardDataWarning {
                    source: "usage.timezone".to_owned(),
                    kind: "invalid_timezone".to_owned(),
                    message: format!("invalid timezone `{raw}`; using UTC"),
                });
                (chrono_tz::UTC, "UTC".to_owned())
            }
        },
        None => {
            warnings.push(DashboardDataWarning {
                source: "usage.timezone".to_owned(),
                kind: "missing_timezone".to_owned(),
                message: "timezone was not provided; using UTC".to_owned(),
            });
            (chrono_tz::UTC, "UTC".to_owned())
        }
    };

    let now_local = now_utc.with_timezone(&tz);

    Ok(UsageClock {
        timezone,
        tz,
        now_utc,
        now_local,
        warnings,
    })
}

pub(crate) fn usage_window_ranges(clock: &UsageClock) -> Result<Vec<UsageWindowRange>, ReadModelError> {
    let today = clock.now_local.date_naive();
    let today_start = local_start_of_day(clock.tz, today)?;
    let week_start = local_start_of_day(clock.tz, today - Duration::days(6))?;
    let month_start = local_start_of_day(clock.tz, today - Duration::days(29))?;
    let now_local = clock.now_local.clone();

    Ok(vec![
        window_range("today", "Today", Some(today_start), now_local.clone(), &clock.timezone),
        window_range("last_7_days", "Last 7 days", Some(week_start), now_local.clone(), &clock.timezone),
        window_range("last_30_days", "Last 30 days", Some(month_start), now_local.clone(), &clock.timezone),
        window_range("all_time", "All time", None, now_local, &clock.timezone),
    ])
}

pub(crate) fn chart_start_utc(clock: &UsageClock, days: i64) -> Result<DateTime<Utc>, ReadModelError> {
    let start_date = clock.now_local.date_naive() - Duration::days(days - 1);
    Ok(local_start_of_day(clock.tz, start_date)?.with_timezone(&Utc))
}

pub(crate) fn local_date_label(ts: &DateTime<Utc>, tz: Tz) -> String {
    ts.with_timezone(&tz).date_naive().format("%Y-%m-%d").to_string()
}

pub(crate) fn local_chart_dates(clock: &UsageClock, days: i64) -> Vec<String> {
    (0..days)
        .map(|offset| {
            (clock.now_local.date_naive() - Duration::days(days - 1 - offset))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect()
}

fn window_range(
    key: &'static str,
    label: &'static str,
    start_local: Option<DateTime<Tz>>,
    end_local: DateTime<Tz>,
    timezone: &str,
) -> UsageWindowRange {
    let since_utc = start_local.as_ref().map(|start| start.with_timezone(&Utc));
    let range_start = start_local.as_ref().map(DateTime::to_rfc3339);
    let range_label = format_range_label(timezone, start_local.as_ref(), &end_local);

    UsageWindowRange {
        key,
        label,
        since_utc,
        until_utc: end_local.with_timezone(&Utc),
        range_start,
        range_end: end_local.to_rfc3339(),
        range_label,
    }
}

fn local_start_of_day(tz: Tz, date: NaiveDate) -> Result<DateTime<Tz>, ReadModelError> {
    match tz.with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0) {
        LocalResult::Single(value) => Ok(value),
        LocalResult::Ambiguous(earliest, _) => Ok(earliest),
        LocalResult::None => Err(ReadModelError::InvalidStartOfDay(format!("{date} in {tz}"))),
    }
}

fn format_range_label(timezone: &str, start: Option<&DateTime<Tz>>, end: &DateTime<Tz>) -> String {
    match start {
        Some(start) if start.date_naive() == end.date_naive() => {
            format!(
                "{timezone} · {} {}-{}",
                start.format("%b %-d"),
                start.format("%H:%M"),
                end.format("%H:%M")
            )
        }
        Some(start) => {
            format!(
                "{timezone} · {} {}-{} {}",
                start.format("%b %-d"),
                start.format("%H:%M"),
                end.format("%b %-d"),
                end.format("%H:%M")
            )
        }
        None => format!(
            "All recorded usage through {} {} · {timezone}",
            end.format("%b %-d"),
            end.format("%H:%M")
        ),
    }
}
```

- [ ] **Step 3: Wire the helper into `usage.rs`**

At the top of `crates/right-dashboard/src/read_model/usage.rs`, replace the chrono import and add the helper module:

```rust
// crates/right-dashboard/src/read_model/usage.rs
use chrono::{DateTime, Duration, Utc};
```

Then add:

```rust
// crates/right-dashboard/src/read_model/usage.rs
#[path = "usage_time.rs"]
mod usage_time;
```

Replace the start of `usage_overview` with:

```rust
// crates/right-dashboard/src/read_model/usage.rs
pub async fn usage_overview(
    conn: &Connection,
    input: UsageOverviewInput,
) -> Result<UsageOverviewResponse, ReadModelError> {
    let clock = usage_time::resolve_usage_clock(&input.generated_at, input.timezone.as_deref())?;
    let unknown_sources = unknown_usage_sources(conn, &clock.now_utc).await?;

    let mut windows = Vec::new();
    for range in usage_time::usage_window_ranges(&clock)? {
        windows.push(build_window(conn, range, &unknown_sources).await?);
    }

    let (daily_series, mut warnings) = build_daily_series(conn, &clock).await?;
    warnings.extend(clock.warnings);
    warnings.extend(unknown_source_warnings(&unknown_sources));
    let source_series = build_source_series(&daily_series, &unknown_sources);

    Ok(UsageOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        timezone: clock.timezone,
        windows,
        selected_window: DEFAULT_CHART_WINDOW.to_owned(),
        daily_series,
        source_series,
        warnings,
    })
}
```

Replace the `build_window` signature and `UsageWindow` construction:

```rust
// crates/right-dashboard/src/read_model/usage.rs
async fn build_window(
    conn: &Connection,
    range: usage_time::UsageWindowRange,
    unknown_sources: &[String],
) -> Result<UsageWindow, ReadModelError> {
    let mut sources = Vec::with_capacity(SOURCES.len() + unknown_sources.len());
    for source in SOURCES {
        sources.push(aggregate_source(conn, source, range.since_utc.as_ref(), &range.until_utc).await?);
    }
    for source in unknown_sources {
        let summary = aggregate_source(conn, source, range.since_utc.as_ref(), &range.until_utc).await?;
        if summary.invocations > 0 {
            sources.push(summary);
        }
    }
    let per_model = aggregate_window_models(&sources);

    Ok(UsageWindow {
        key: range.key.to_owned(),
        label: range.label.to_owned(),
        range_start: range.range_start,
        range_end: range.range_end,
        range_label: range.range_label,
        total_cost_usd: sources.iter().map(|source| source.cost_usd).sum(),
        subscription_cost_usd: sources.iter().map(|source| source.subscription_cost_usd).sum(),
        api_cost_usd: sources.iter().map(|source| source.api_cost_usd).sum(),
        turns: sources.iter().map(|source| source.turns).sum(),
        invocations: sources.iter().map(|source| source.invocations).sum(),
        input_tokens: sources.iter().map(|source| source.input_tokens).sum(),
        output_tokens: sources.iter().map(|source| source.output_tokens).sum(),
        cache_creation_tokens: sources.iter().map(|source| source.cache_creation_tokens).sum(),
        cache_read_tokens: sources.iter().map(|source| source.cache_read_tokens).sum(),
        web_search_requests: sources.iter().map(|source| source.web_search_requests).sum(),
        web_fetch_requests: sources.iter().map(|source| source.web_fetch_requests).sum(),
        per_model,
        sources,
        budget_skip_count: budget_skip_count(conn, range.since_utc.as_ref(), &range.until_utc).await?,
    })
}
```

Replace the `build_daily_series` header and date setup:

```rust
// crates/right-dashboard/src/read_model/usage.rs
async fn build_daily_series(
    conn: &Connection,
    clock: &usage_time::UsageClock,
) -> Result<(Vec<UsageDailyPoint>, Vec<DashboardDataWarning>), ReadModelError> {
    let mut warnings = Vec::new();
    let chart_start_utc = usage_time::chart_start_utc(clock, DAILY_SERIES_DAYS)?;
    let coarse_since = (chart_start_utc - Duration::days(1)).to_rfc3339();
    let coarse_until = (clock.now_utc + Duration::days(1)).to_rfc3339();

    let mut points = usage_time::local_chart_dates(clock, DAILY_SERIES_DAYS)
        .into_iter()
        .map(|date| UsageDailyPoint {
            date,
            total_cost_usd: 0.0,
            subscription_cost_usd: 0.0,
            api_cost_usd: 0.0,
            turns: 0,
            invocations: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            web_search_requests: 0,
            web_fetch_requests: 0,
            sources: Vec::new(),
            models: Vec::new(),
        })
        .collect::<Vec<_>>();
```

Inside `build_daily_series`, replace the precise filter and date extraction:

```rust
// crates/right-dashboard/src/read_model/usage.rs
let event_at = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
if event_at < chart_start_utc || event_at > clock.now_utc {
    continue;
}

let date = usage_time::local_date_label(&event_at, clock.tz);
```

- [ ] **Step 4: Update existing `UsageOverviewInput` test constructors**

In `crates/right-dashboard/src/read_model/usage.rs`, update every `UsageOverviewInput` literal to include:

```rust
// crates/right-dashboard/src/read_model/usage.rs
timezone: Some("UTC".to_owned()),
```

Use `timezone: Some("UTC".to_owned())` for existing tests so prior UTC expectations remain stable. Do not change unrelated assertions.

- [ ] **Step 5: Run targeted Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview
devenv shell -- cargo test -p right-dashboard usage_local_time_tests
```

Expected: both commands PASS.

- [ ] **Step 6: Commit backend read-model implementation**

```bash
git add Cargo.toml crates/right-dashboard/Cargo.toml \
        crates/right-dashboard/src/api_types.rs \
        crates/right-dashboard/src/read_model/usage.rs \
        crates/right-dashboard/src/read_model/usage_time.rs \
        crates/right-dashboard/src/read_model/usage_local_time_tests.rs
git commit -m "feat(dashboard): compute usage windows in viewer timezone"
```

## Task 3: Dashboard Route Query Parameter

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Add the failing route test**

In `crates/bot/src/telegram/dashboard.rs`, add these assertions to `usage_returns_structured_windows_for_authorized_user` after `assert_eq!(body["agent"], "alpha");`:

```rust
// crates/bot/src/telegram/dashboard.rs
assert_eq!(body["timezone"], "UTC");
assert!(body["windows"][0]["range_start"].is_string());
assert!(body["windows"][0]["range_end"].is_string());
assert!(body["windows"][0]["range_label"].as_str().unwrap().contains("UTC"));
```

Then add a new test near it:

```rust
// crates/bot/src/telegram/dashboard.rs
#[tokio::test]
async fn usage_accepts_timezone_query_for_authorized_user() {
    let temp = tempfile::tempdir().expect("tempdir");
    let conn = right_db::open_connection(temp.path(), true)
        .await
        .expect("open migrated db");
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (
            '2026-06-03T20:00:00Z', 'interactive', 1, 0, NULL, 's1',
            0.15, 1, 10, 20, 0, 0, 0, 0,
            '{\"sonnet\":{\"costUSD\":0.15}}', 'none'
         )",
        [],
    )
    .await
    .unwrap();

    let (status, body) = get_json(
        "/dashboard/alpha/api/v1/usage?timezone=Asia%2FDubai",
        Some(signed_init_data(42)),
        temp.path().to_path_buf(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["timezone"], "Asia/Dubai");
    assert!(body["windows"][0]["range_label"].as_str().unwrap().starts_with("Asia/Dubai"));
}
```

- [ ] **Step 2: Run the failing bot test**

Run:

```bash
devenv shell -- cargo test -p bot usage_accepts_timezone_query_for_authorized_user
```

Expected: FAIL because the handler does not parse the query and still constructs `UsageOverviewInput` without timezone.

- [ ] **Step 3: Implement query parsing**

Modify imports in `crates/bot/src/telegram/dashboard.rs`:

```rust
// crates/bot/src/telegram/dashboard.rs
use axum::extract::{Path as AxumPath, Query, State};
```

Add this struct near `DashboardState`:

```rust
// crates/bot/src/telegram/dashboard.rs
#[derive(Debug, Default, serde::Deserialize)]
struct UsageQuery {
    timezone: Option<String>,
}
```

Change the handler signature and input:

```rust
// crates/bot/src/telegram/dashboard.rs
async fn handle_usage_overview(
    AxumPath(agent): AxumPath<String>,
    Query(query): Query<UsageQuery>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let input = UsageOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        timezone: query.timezone,
    };
```

- [ ] **Step 4: Run targeted bot tests**

Run:

```bash
devenv shell -- cargo test -p bot usage_returns_structured_windows_for_authorized_user usage_accepts_timezone_query_for_authorized_user
```

Expected: PASS.

- [ ] **Step 5: Commit route change**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "feat(dashboard): accept usage timezone query"
```

## Task 4: Frontend API and Selected-Day Range Helpers

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/api.ts`
- Create: `crates/right-dashboard/frontend/src/api.test.ts`
- Create: `crates/right-dashboard/frontend/src/views/usageDayRange.ts`
- Create: `crates/right-dashboard/frontend/src/views/usageDayRange.test.ts`

- [ ] **Step 1: Extend frontend types**

In `crates/right-dashboard/frontend/src/types.ts`, update `UsageOverviewResponse` and `UsageWindow`:

```ts
// crates/right-dashboard/frontend/src/types.ts
export interface UsageOverviewResponse {
  agent: string
  generated_at: string
  timezone: string
  windows: UsageWindow[]
  selected_window: string
  daily_series: UsageDailyPoint[]
  source_series: UsageSourceSeries[]
  warnings: DashboardDataWarning[]
}

export interface UsageWindow {
  key: string
  label: string
  range_start: string | null
  range_end: string
  range_label: string
  sources: UsageSourceSummary[]
  total_cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  per_model: UsageModelSummary[]
  budget_skip_count: number
}
```

- [ ] **Step 2: Add failing API test**

Create `crates/right-dashboard/frontend/src/api.test.ts`:

```ts
// crates/right-dashboard/frontend/src/api.test.ts
import { afterEach, describe, expect, it, vi } from 'vitest'

import { browserUsageTimezone, usageOverview } from './api'

function usagePayload() {
  return {
    agent: 'right',
    generated_at: '2026-06-04T12:00:00Z',
    timezone: 'Asia/Dubai',
    windows: [],
    selected_window: 'last_30_days',
    daily_series: [],
    source_series: [],
    warnings: [],
  }
}

describe('usageOverview', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('sends the browser timezone as a query parameter', async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify(usagePayload()), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })
    vi.spyOn(Intl, 'DateTimeFormat').mockReturnValue({
      resolvedOptions: () => ({ timeZone: 'Asia/Dubai' }),
    } as Intl.DateTimeFormat)

    await usageOverview()

    expect(fetchMock).toHaveBeenCalledOnce()
    expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=Asia%2FDubai')
  })

  it('falls back to UTC if Intl returns no timezone', () => {
    vi.spyOn(Intl, 'DateTimeFormat').mockReturnValue({
      resolvedOptions: () => ({ timeZone: undefined }),
    } as Intl.DateTimeFormat)

    expect(browserUsageTimezone()).toBe('UTC')
  })
})
```

- [ ] **Step 3: Run failing API test**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/api.test.ts'
```

Expected: FAIL because `browserUsageTimezone` is not exported and `usageOverview` does not add `timezone`.

- [ ] **Step 4: Implement API timezone query**

In `crates/right-dashboard/frontend/src/api.ts`, replace `usageOverview` with:

```ts
// crates/right-dashboard/frontend/src/api.ts
export function browserUsageTimezone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

export function usageOverview(timezone: string = browserUsageTimezone()): Promise<UsageOverviewResponse> {
  const params = new URLSearchParams({ timezone })
  return requestJson<UsageOverviewResponse>(`api/v1/usage?${params.toString()}`)
}
```

- [ ] **Step 5: Add selected-day range helper tests**

Create `crates/right-dashboard/frontend/src/views/usageDayRange.test.ts`:

```ts
// crates/right-dashboard/frontend/src/views/usageDayRange.test.ts
import { describe, expect, it } from 'vitest'

import { selectedDayRangeLabel } from './usageDayRange'

describe('selectedDayRangeLabel', () => {
  it('formats past local days as full-day ranges', () => {
    expect(selectedDayRangeLabel('2026-06-03', 'Asia/Dubai', '2026-06-04T16:47:36Z'))
      .toBe('Asia/Dubai · Jun 3 00:00-23:59')
  })

  it('formats the current local day through generated-at local time', () => {
    expect(selectedDayRangeLabel('2026-06-04', 'Asia/Dubai', '2026-06-04T16:47:36Z'))
      .toBe('Asia/Dubai · Jun 4 00:00-20:47')
  })

  it('returns null when no date is selected', () => {
    expect(selectedDayRangeLabel(null, 'Asia/Dubai', '2026-06-04T16:47:36Z')).toBeNull()
  })
})
```

- [ ] **Step 6: Run failing range helper test**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/usageDayRange.test.ts'
```

Expected: FAIL because the helper file does not exist.

- [ ] **Step 7: Implement selected-day range helper**

Create `crates/right-dashboard/frontend/src/views/usageDayRange.ts`:

```ts
// crates/right-dashboard/frontend/src/views/usageDayRange.ts
const MONTH = new Intl.DateTimeFormat('en-US', { month: 'short' })

function pad2(value: number): string {
  return String(value).padStart(2, '0')
}

function localParts(date: Date, timezone: string): { year: number; month: number; day: number; hour: number; minute: number } {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date)

  const get = (type: string) => Number(parts.find((part) => part.type === type)?.value ?? 0)
  return {
    year: get('year'),
    month: get('month'),
    day: get('day'),
    hour: get('hour'),
    minute: get('minute'),
  }
}

function monthName(month: number): string {
  return MONTH.format(new Date(Date.UTC(2026, month - 1, 1)))
}

export function selectedDayRangeLabel(
  selectedDate: string | null,
  timezone: string,
  generatedAt: string,
): string | null {
  if (selectedDate === null) {
    return null
  }

  const generated = localParts(new Date(generatedAt), timezone)
  const currentDate = `${generated.year}-${pad2(generated.month)}-${pad2(generated.day)}`
  const [, monthRaw, dayRaw] = selectedDate.split('-')
  const month = Number(monthRaw)
  const day = Number(dayRaw)
  const end = selectedDate === currentDate ? `${pad2(generated.hour)}:${pad2(generated.minute)}` : '23:59'

  return `${timezone} · ${monthName(month)} ${day} 00:00-${end}`
}
```

- [ ] **Step 8: Run frontend helper tests**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/api.test.ts src/views/usageDayRange.test.ts'
```

Expected: PASS.

- [ ] **Step 9: Commit frontend API/helper changes**

```bash
git add crates/right-dashboard/frontend/src/types.ts \
        crates/right-dashboard/frontend/src/api.ts \
        crates/right-dashboard/frontend/src/api.test.ts \
        crates/right-dashboard/frontend/src/views/usageDayRange.ts \
        crates/right-dashboard/frontend/src/views/usageDayRange.test.ts
git commit -m "feat(dashboard): request usage in browser timezone"
```

## Task 5: Usage UI Range and Legend Rendering

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.test.ts`
- Modify: `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue`
- Modify: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
- Modify: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts`

- [ ] **Step 1: Update UsageView test stubs**

In `crates/right-dashboard/frontend/src/views/UsageView.test.ts`, update `windowStub` and `usageStub` defaults:

```ts
// crates/right-dashboard/frontend/src/views/UsageView.test.ts
function windowStub(overrides: Partial<UsageWindow> = {}): UsageWindow {
  return {
    key: '7d',
    label: 'Last 7 days',
    range_start: '2025-12-26T00:00:00+04:00',
    range_end: '2026-01-01T04:00:00+04:00',
    range_label: 'Asia/Dubai · Dec 26 00:00-Jan 1 04:00',
    sources: [sourceSummaryStub()],
    total_cost_usd: 1.0,
    subscription_cost_usd: 0,
    api_cost_usd: 1.0,
    turns: 5,
    invocations: 5,
    input_tokens: 1000,
    output_tokens: 500,
    cache_creation_tokens: 200,
    cache_read_tokens: 800,
    web_search_requests: 0,
    web_fetch_requests: 0,
    per_model: [],
    budget_skip_count: 0,
    ...overrides,
  }
}

function usageStub(overrides: Partial<UsageOverviewResponse> = {}): UsageOverviewResponse {
  return {
    agent: 'test-agent',
    generated_at: '2026-01-01T00:00:00Z',
    timezone: 'Asia/Dubai',
    windows: [windowStub()],
    selected_window: '7d',
    daily_series: [],
    source_series: [],
    warnings: [],
    ...overrides,
  }
}
```

- [ ] **Step 2: Add failing UsageView rendering assertions**

In the existing `UsageView token legend and per-source TokenLine` test, replace the assertions with:

```ts
// crates/right-dashboard/frontend/src/views/UsageView.test.ts
expect((html.match(/token-legend/g) ?? []).length).toBe(2)
expect((html.match(/is-sticky/g) ?? []).length).toBe(1)
expect(html).toContain('token-line')
expect(html).toContain('interactive')
expect(html).toContain('Asia/Dubai · Dec 26 00:00-Jan 1 04:00')
expect(html).toContain('lg-create')
expect(html).toContain('lg-read')
expect(html).not.toContain('cache-subline')
```

Add a selected-day range test:

```ts
// crates/right-dashboard/frontend/src/views/UsageView.test.ts
it('passes the selected local day range to the breakdown panel', async () => {
  const html = await render({
    usage: usageStub({
      generated_at: '2026-06-04T16:47:36Z',
      timezone: 'Asia/Dubai',
      daily_series: [{
        date: '2026-06-04',
        total_cost_usd: 1,
        subscription_cost_usd: 1,
        api_cost_usd: 0,
        turns: 1,
        invocations: 1,
        input_tokens: 10,
        output_tokens: 20,
        cache_creation_tokens: 5,
        cache_read_tokens: 40,
        web_search_requests: 0,
        web_fetch_requests: 0,
        sources: [],
        models: [],
      }],
    }),
    loading: false,
    error: null,
  })

  expect(html).toContain('Asia/Dubai · Jun 4 00:00-20:47')
})
```

- [ ] **Step 3: Run failing UsageView tests**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts'
```

Expected: FAIL because only one `TokenLegend` renders and window/selected-day range labels are not rendered.

- [ ] **Step 4: Make TokenLegend sticky only when requested**

In `crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue`, replace the whole file with:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue -->
<script setup lang="ts">
withDefaults(defineProps<{ sticky?: boolean }>(), {
  sticky: true,
})
</script>

<template>
  <div class="token-legend" :class="{ 'is-sticky': sticky }">
    <span class="lg lg-input">input</span>
    <span class="lg lg-output">output</span>
    <span class="lg lg-create">cache create</span>
    <span class="lg lg-read">cache read</span>
  </div>
</template>

<style scoped>
.token-legend {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 8px 0;
  padding: 8px 0;
}

.token-legend.is-sticky {
  position: sticky;
  bottom: 0;
  z-index: 15;
  margin: 8px -12px 0;
  padding: 8px 12px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
}

@media (max-width: 560px) {
  .token-legend.is-sticky {
    /* clear the fixed mobile nav bar (.app-shell reserves 78px for it) */
    bottom: calc(78px + env(safe-area-inset-bottom));
  }
}
.lg {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.lg::before {
  content: '';
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--dot);
}
.lg-input { --dot: var(--token-input); }
.lg-output { --dot: var(--token-output); }
.lg-create { --dot: var(--token-create); }
.lg-read { --dot: var(--token-read); }
</style>
```

- [ ] **Step 5: Update UsageView implementation**

In `crates/right-dashboard/frontend/src/views/UsageView.vue`, add the import:

```ts
// crates/right-dashboard/frontend/src/views/UsageView.vue
import { selectedDayRangeLabel } from './usageDayRange'
```

Add this computed value after `selectedPoint`:

```ts
// crates/right-dashboard/frontend/src/views/UsageView.vue
const selectedRangeLabel = computed(() =>
  selectedDayRangeLabel(selectedDate.value, props.usage?.timezone ?? 'UTC', props.usage?.generated_at ?? new Date().toISOString()),
)
```

In the template, insert the top legend after warnings and before the chart section:

```vue
<!-- crates/right-dashboard/frontend/src/views/UsageView.vue -->
<TokenLegend :sticky="false" />
```

Change the `UsageBreakdown` call:

```vue
<!-- crates/right-dashboard/frontend/src/views/UsageView.vue -->
<UsageBreakdown :point="selectedPoint" :range-label="selectedRangeLabel" />
```

In each window header title block, add the range subline under the `h2`:

```vue
<!-- crates/right-dashboard/frontend/src/views/UsageView.vue -->
<p class="eyebrow">{{ window.key }}</p>
<h2>{{ window.label }}</h2>
<p class="muted-line">{{ window.range_label }}</p>
```

- [ ] **Step 6: Add failing UsageBreakdown test**

In `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts`, add:

```ts
// crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts
it('renders the selected local day range when provided', async () => {
  const html = await render(point(), 'Asia/Dubai · Jun 4 00:00-20:47')
  expect(html).toContain('Asia/Dubai · Jun 4 00:00-20:47')
})
```

Update that file's local render helper to accept a range label:

```ts
// crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts
async function render(p: UsageDailyPoint | null = point(), rangeLabel: string | null = null) {
  const app = createSSRApp({ render: () => h(UsageBreakdown, { point: p, rangeLabel }) })
  return renderToString(app)
}
```

- [ ] **Step 7: Run failing UsageBreakdown test**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/components/charts/UsageBreakdown.test.ts'
```

Expected: FAIL because `UsageBreakdown` does not accept or render `rangeLabel`.

- [ ] **Step 8: Update UsageBreakdown implementation**

In `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`, change props:

```ts
// crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue
defineProps<{
  point: UsageDailyPoint | null
  rangeLabel?: string | null
}>()
```

In the header block, add:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue -->
<p v-if="rangeLabel" class="muted-line">{{ rangeLabel }}</p>
```

Place it under the `<h2>{{ point?.date ?? 'None selected' }}</h2>` line.

- [ ] **Step 9: Run frontend usage tests and typecheck**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm exec vitest run src/views/UsageView.test.ts src/components/charts/UsageBreakdown.test.ts src/views/usageDayRange.test.ts && pnpm typecheck'
```

Expected: all selected tests PASS and `vue-tsc --noEmit` reports no errors.

- [ ] **Step 10: Commit Usage UI changes**

```bash
git add crates/right-dashboard/frontend/src/views/UsageView.vue \
        crates/right-dashboard/frontend/src/views/UsageView.test.ts \
        crates/right-dashboard/frontend/src/components/charts/TokenLegend.vue \
        crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue \
        crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.test.ts
git commit -m "feat(dashboard): show usage ranges and token legends"
```

## Task 6: Architecture Doc Check and Final Verification

**Files:**
- Read: `docs/architecture/modules.md`
- Modify only if drifted: `docs/architecture/modules.md`

- [ ] **Step 1: Check dashboard architecture docs**

Read `docs/architecture/modules.md` and look for the dashboard/read-model section. If it already describes the dashboard at the same level of detail and does not mention UTC-only Usage semantics, make no edit.

If it needs a small update, add exactly this sentence to the dashboard/read-model bullet:

```markdown
Usage read models accept a viewer timezone and bucket Usage-tab windows by that local calendar before converting bounds back to UTC for storage filtering.
```

- [ ] **Step 2: Run full frontend verification**

Run:

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck && pnpm build'
```

Expected: vitest PASS, typecheck PASS, Vite build PASS.

- [ ] **Step 3: Run targeted Rust dashboard/bot verification**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview
devenv shell -- cargo test -p bot usage_returns_structured_windows_for_authorized_user usage_accepts_timezone_query_for_authorized_user
```

Expected: both Rust test commands PASS.

- [ ] **Step 4: Run mandatory final workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: full workspace test suite PASS. If frontend build artifacts are generated by Cargo build/test flow, include only intentional source or generated asset changes in the final commit.

- [ ] **Step 5: Commit docs/final verification changes if needed**

If `docs/architecture/modules.md` was changed:

```bash
git add docs/architecture/modules.md
git commit -m "docs(architecture): note timezone-aware usage buckets"
```

If no architecture doc changed, do not create an empty commit.
