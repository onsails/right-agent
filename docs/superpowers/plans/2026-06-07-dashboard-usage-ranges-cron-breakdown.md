# Dashboard Usage Ranges and Cron Job Breakdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Usage dashboard show one selected range at a time, defaulting to last 7 days, and add a selected-range cron-job usage breakdown.

**Architecture:** The dashboard Usage API becomes selected-range driven: the request includes `range`, the read model computes one effective local-calendar window, and the frontend renders only that selected window. The response keeps temporary one-element legacy fields for compatibility while new UI code reads `window`, `selected_range`, and `cron_jobs`.

**Tech Stack:** Rust 2024, `right-dashboard` read models over `right-db`, Axum route glue in `right-bot`, Vue 3 Composition API, Vitest SSR tests, Cargo workspace tests via `devenv shell --`.

---

## Execution Notes

- Before editing Rust, invoke `rust-dev:rust-dev` if that skill is available in the executor environment. When this plan was written, that skill was not installed in the visible Codex skill set, so the plan follows `AGENTS.rust.md` directly.
- Do not create a branch unless explicitly asked.
- Keep new Rust tests out of the already-large `usage.rs`; create sibling test files and register them with explicit `#[path = "<test-file>.rs"]` module declarations.
- Use TDD: write each failing test first, run it and confirm the expected failure, then implement.
- Use targeted tests while iterating. Run `devenv shell -- cargo test --workspace` only at final verification.
- Do not touch existing untracked files unrelated to this feature.

## File Structure

- Modify `crates/right-dashboard/src/api_types.rs`: add selected-range response fields and `UsageCronJobSummary`.
- Modify `crates/right-dashboard/src/read_model/usage_time.rs`: add usage range parsing and selected range window/date helpers.
- Modify `crates/right-dashboard/src/read_model/usage.rs`: use one selected window, selected daily series, selected source series, and selected cron-job aggregation.
- Create `crates/right-dashboard/src/read_model/usage_range_tests.rs`: selected range and all-time read-model regressions.
- Create `crates/right-dashboard/src/read_model/usage_cron_jobs_tests.rs`: cron job grouping regressions.
- Modify `crates/bot/src/telegram/dashboard.rs`: accept and pass `range` query param; extend route tests.
- Modify `crates/right-dashboard/frontend/src/types.ts`: add `UsageRange`, `window`, `selected_range`, and `UsageCronJobSummary`.
- Create `crates/right-dashboard/frontend/src/views/usageRanges.ts`: canonical range options and validation helpers.
- Modify `crates/right-dashboard/frontend/src/api.ts` and `src/api.test.ts`: send `range`.
- Modify `crates/right-dashboard/frontend/src/views/UsageContainer.vue`: own requested range and trigger refetches.
- Create `crates/right-dashboard/frontend/src/views/UsageContainer.test.ts`: default range SSR coverage.
- Modify `crates/right-dashboard/frontend/src/views/UsageView.vue` and `UsageView.test.ts`: segmented control, one selected window, cron rows.
- Modify `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`: make title configurable.
- Modify `docs/architecture/modules.md`: keep the `right-dashboard/read_model/usage.rs` description in sync.

### Task 0: Baseline Verification

**Files:**
- Read: `docs/superpowers/specs/2026-06-07-dashboard-usage-ranges-cron-breakdown-design.md`
- Read: `docs/architecture/modules.md`

- [ ] **Step 1: Confirm working tree state**

Run:

```bash
devenv shell -- git status --short
```

Expected: only unrelated pre-existing untracked files, or a clean tree. Record anything pre-existing before editing.

- [ ] **Step 2: Run targeted Rust baseline**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview_builds_daily_series_for_last_30_days
```

Expected: PASS. If it fails before edits, record the failure and do not treat it as caused by this feature.

- [ ] **Step 3: Run targeted frontend baseline**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/api.test.ts src/views/UsageView.test.ts src/components/charts/UsageBreakdown.test.ts
```

Expected: PASS. If it fails before edits, record the failure.

### Task 1: Backend Contract and Range Tests

**Files:**
- Create: `crates/right-dashboard/src/read_model/usage_range_tests.rs`
- Modify: `crates/right-dashboard/src/read_model/usage.rs`
- Modify: `crates/right-dashboard/src/api_types.rs`

- [ ] **Step 1: Write failing selected-range tests**

Create `crates/right-dashboard/src/read_model/usage_range_tests.rs`:

```rust
use super::*;
use right_db::{open_connection, params};
use tempfile::tempdir;

async fn insert_usage(conn: &right_db::Connection, ts: &str, source: &str, cost: f64) {
    let model_json = format!(
        r#"{{"sonnet":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":5,"cacheReadInputTokens":40}}}}"#
    );
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, NULL, ?3, ?4, 1, 10, 20, 5, 40, 0, 0, ?5, 'none')",
        params![ts, source, format!("{source}-{ts}"), cost, model_json],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn usage_overview_defaults_to_last_7_days() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-05-28T00:00:00Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-05-29T00:00:00Z", "interactive", 1.00).await;
    insert_usage(&conn, "2026-06-04T00:00:00Z", "interactive", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_7_days");
    assert_eq!(response.selected_window, "last_7_days");
    assert_eq!(response.window.key, "last_7_days");
    assert_eq!(response.windows.len(), 1);
    assert_eq!(response.windows[0].key, "last_7_days");
    assert_eq!(response.daily_series.len(), 7);
    assert_eq!(response.daily_series.first().unwrap().date, "2026-05-29");
    assert_eq!(response.daily_series.last().unwrap().date, "2026-06-04");
    assert!((response.window.total_cost_usd - 3.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_last_3_days_uses_local_calendar_window() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-01T19:59:59Z", "interactive", 9.99).await;
    insert_usage(&conn, "2026-06-01T20:00:00Z", "interactive", 1.00).await;
    insert_usage(&conn, "2026-06-04T16:47:36Z", "interactive", 2.00).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T16:47:36Z".to_owned(),
            timezone: Some("Asia/Dubai".to_owned()),
            range: Some("last_3_days".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_3_days");
    assert_eq!(response.window.key, "last_3_days");
    assert_eq!(response.window.range_start.as_deref(), Some("2026-06-02T00:00:00+04:00"));
    assert_eq!(
        response.daily_series.iter().map(|point| point.date.as_str()).collect::<Vec<_>>(),
        vec!["2026-06-02", "2026-06-03", "2026-06-04"]
    );
    assert!((response.window.total_cost_usd - 3.00).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_invalid_range_falls_back_to_last_7_days_with_warning() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("forever".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "last_7_days");
    assert_eq!(response.window.key, "last_7_days");
    assert!(response.warnings.iter().any(|warning| {
        warning.source == "usage.range"
            && warning.kind == "invalid_range"
            && warning.message.contains("forever")
    }));
}

#[tokio::test]
async fn usage_overview_all_time_buckets_from_first_recorded_usage() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-05-01T08:00:00Z", "interactive", 0.10).await;
    insert_usage(&conn, "2026-05-03T08:00:00Z", "cron", 0.20).await;
    insert_usage(&conn, "2026-05-07T08:00:00Z", "interactive", 9.99).await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-05T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("all_time".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.selected_range, "all_time");
    assert_eq!(response.window.key, "all_time");
    assert_eq!(response.window.range_start, None);
    assert_eq!(
        response.daily_series.iter().map(|point| point.date.as_str()).collect::<Vec<_>>(),
        vec!["2026-05-01", "2026-05-02", "2026-05-03", "2026-05-04", "2026-05-05"]
    );
    assert!((response.window.total_cost_usd - 0.30).abs() < 1e-9);
}
```

Register the new test module at the bottom of `crates/right-dashboard/src/read_model/usage.rs`:

```rust
#[cfg(test)]
#[path = "usage_range_tests.rs"]
mod usage_range_tests;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_range_tests
```

Expected: FAIL to compile because `UsageOverviewInput.range`, `UsageOverviewResponse.selected_range`, `UsageOverviewResponse.window`, and range behavior do not exist yet.

- [ ] **Step 3: Add response and input contract fields**

Modify `crates/right-dashboard/src/api_types.rs`.

Add `selected_range`, `window`, and `cron_jobs` to `UsageOverviewResponse` while keeping compatibility fields:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub timezone: String,
    pub selected_range: String,
    pub window: UsageWindow,
    pub windows: Vec<UsageWindow>,
    pub selected_window: String,
    pub daily_series: Vec<UsageDailyPoint>,
    pub source_series: Vec<UsageSourceSeries>,
    pub cron_jobs: Vec<UsageCronJobSummary>,
    pub warnings: Vec<DashboardDataWarning>,
}
```

Add the cron summary type after `UsageSourceSummary`:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageCronJobSummary {
    pub job_name: String,
    pub cost_usd: f64,
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
}
```

Update every `UsageOverviewResponse` fixture in `api_types.rs` using this pattern:

```rust
let usage_window = UsageWindow {
    key: "last_7_days".to_owned(),
    label: "Last 7 days".to_owned(),
    range_start: Some("2026-05-26T00:00:00+00:00".to_owned()),
    range_end: "2026-06-01T12:00:00+00:00".to_owned(),
    range_label: "UTC · May 26 00:00-Jun 1 12:00".to_owned(),
    sources: Vec::new(),
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
    per_model: Vec::new(),
    budget_skip_count: 0,
};
UsageOverviewResponse {
    agent: "agent".to_owned(),
    generated_at: "2026-06-01T12:00:00Z".to_owned(),
    timezone: "UTC".to_owned(),
    selected_range: "last_7_days".to_owned(),
    window: usage_window.clone(),
    windows: vec![usage_window],
    selected_window: "last_7_days".to_owned(),
    daily_series: Vec::new(),
    source_series: Vec::new(),
    cron_jobs: Vec::new(),
    warnings: Vec::new(),
}
```

Modify `crates/right-dashboard/src/read_model/usage.rs`:

```rust
use crate::api_types::{
    CostSeriesPoint, DashboardDataWarning, UsageCronJobSummary, UsageDailyPoint,
    UsageModelSummary, UsageOverviewResponse, UsageSourcePoint, UsageSourceSeries,
    UsageSourceSummary, UsageWindow,
};
```

Add `range` to `UsageOverviewInput`:

```rust
pub struct UsageOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub timezone: Option<String>,
    pub range: Option<String>,
}
```

Update existing test calls in `usage.rs` and `usage_local_time_tests.rs` to add:

```rust
range: None,
```

- [ ] **Step 4: Run tests to verify contract compile progresses but behavior still fails**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_range_tests
```

Expected: FAIL because response construction and selected range behavior are still not implemented.

### Task 2: Selected Range Resolver and Daily Series

**Files:**
- Modify: `crates/right-dashboard/src/read_model/usage_time.rs`
- Modify: `crates/right-dashboard/src/read_model/usage.rs`
- Modify: `crates/right-dashboard/src/read_model/usage_local_time_tests.rs`
- Modify: `crates/right-dashboard/src/read_model/usage_range_tests.rs`

- [ ] **Step 1: Add range resolver to `usage_time.rs`**

Add this near the existing constants/types in `crates/right-dashboard/src/read_model/usage_time.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UsageRangeKey {
    Today,
    Last3Days,
    Last7Days,
    Last30Days,
    AllTime,
}

impl UsageRangeKey {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Last3Days => "last_3_days",
            Self::Last7Days => "last_7_days",
            Self::Last30Days => "last_30_days",
            Self::AllTime => "all_time",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Last3Days => "Last 3 days",
            Self::Last7Days => "Last 7 days",
            Self::Last30Days => "Last 30 days",
            Self::AllTime => "All time",
        }
    }
}

pub(super) const DEFAULT_USAGE_RANGE: UsageRangeKey = UsageRangeKey::Last7Days;

pub(super) fn resolve_usage_range(
    requested_range: Option<&str>,
) -> (UsageRangeKey, Vec<DashboardDataWarning>) {
    let Some(raw) = requested_range.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return (DEFAULT_USAGE_RANGE, Vec::new());
    };

    match raw {
        "today" => (UsageRangeKey::Today, Vec::new()),
        "last_3_days" => (UsageRangeKey::Last3Days, Vec::new()),
        "last_7_days" => (UsageRangeKey::Last7Days, Vec::new()),
        "last_30_days" => (UsageRangeKey::Last30Days, Vec::new()),
        "all_time" => (UsageRangeKey::AllTime, Vec::new()),
        invalid => (
            DEFAULT_USAGE_RANGE,
            vec![DashboardDataWarning {
                source: "usage.range".to_owned(),
                kind: "invalid_range".to_owned(),
                message: format!("invalid usage range `{invalid}`; falling back to last_7_days"),
            }],
        ),
    }
}
```

Replace `usage_window_ranges` with a selected-range helper:

```rust
pub(super) fn usage_window_range(
    clock: &UsageClock,
    range: UsageRangeKey,
) -> Result<UsageWindowRange, ReadModelError> {
    let today = clock.now_local.date_naive();
    let since_local = match range {
        UsageRangeKey::Today => Some(local_start_of_day(today, &clock.tz)?),
        UsageRangeKey::Last3Days => Some(local_start_of_day(today - Duration::days(2), &clock.tz)?),
        UsageRangeKey::Last7Days => Some(local_start_of_day(today - Duration::days(6), &clock.tz)?),
        UsageRangeKey::Last30Days => Some(local_start_of_day(today - Duration::days(29), &clock.tz)?),
        UsageRangeKey::AllTime => None,
    };

    Ok(window_range(clock, range.key(), range.label(), since_local))
}
```

Add a helper for all-time chart starts:

```rust
pub(super) fn local_date_start_utc(
    date: NaiveDate,
    tz: &Tz,
) -> Result<DateTime<Utc>, ReadModelError> {
    Ok(local_start_of_day(date, tz)?.with_timezone(&Utc))
}
```

Keep `chart_start_utc` only if another caller still uses it; otherwise remove it with the old fixed 30-day call site.

- [ ] **Step 2: Implement selected range response in `usage.rs`**

Replace the top of `usage_overview` in `crates/right-dashboard/src/read_model/usage.rs` with this structure:

```rust
pub async fn usage_overview(
    conn: &Connection,
    input: UsageOverviewInput,
) -> Result<UsageOverviewResponse, ReadModelError> {
    let clock = usage_time::resolve_usage_clock(&input.generated_at, input.timezone.as_deref())?;
    let (range_key, range_warnings) = usage_time::resolve_usage_range(input.range.as_deref());
    let range = usage_time::usage_window_range(&clock, range_key)?;
    let unknown_sources = unknown_usage_sources(conn, &clock.now_utc).await?;

    let window = build_window(conn, &range, &unknown_sources).await?;
    let (daily_series, mut warnings) = build_daily_series(conn, &clock, &range).await?;
    let source_series = build_source_series(&daily_series, &unknown_sources);
    let cron_jobs = build_cron_jobs(conn, range.since_utc.as_ref(), &range.until_utc).await?;

    warnings.extend(clock.warnings);
    warnings.extend(range_warnings);
    warnings.extend(unknown_source_warnings(&unknown_sources));

    let selected_range = range.key.to_owned();

    Ok(UsageOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at,
        timezone: clock.timezone,
        selected_range: selected_range.clone(),
        window: window.clone(),
        windows: vec![window],
        selected_window: selected_range,
        daily_series,
        source_series,
        cron_jobs,
        warnings,
    })
}
```

At this point add a temporary stub below `build_source_series` so the code compiles before Task 3:

```rust
async fn build_cron_jobs(
    _conn: &Connection,
    _since: Option<&DateTime<Utc>>,
    _until: &DateTime<Utc>,
) -> Result<Vec<UsageCronJobSummary>, ReadModelError> {
    Ok(Vec::new())
}
```

- [ ] **Step 3: Refactor daily series to selected range**

Change the `build_daily_series` signature:

```rust
async fn build_daily_series(
    conn: &Connection,
    clock: &usage_time::UsageClock,
    range: &usage_time::UsageWindowRange,
) -> Result<(Vec<UsageDailyPoint>, Vec<DashboardDataWarning>), ReadModelError> {
```

Replace the fixed `chart_start_utc` and `local_chart_dates` setup with:

```rust
    let Some(chart_start_utc) = chart_start_for_range(conn, clock, range).await? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let coarse_since = (chart_start_utc - Duration::days(1)).to_rfc3339();
    let coarse_until = (clock.now_utc + Duration::days(1)).to_rfc3339();

    let mut points = usage_time::local_chart_dates_from(clock, chart_start_utc)?
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

Add this helper to `usage_time.rs`:

```rust
pub(super) fn local_chart_dates_from(
    clock: &UsageClock,
    chart_start_utc: DateTime<Utc>,
) -> Result<Vec<String>, ReadModelError> {
    let start_date = chart_start_utc.with_timezone(&clock.tz).date_naive();
    let end_date = clock.now_local.date_naive();
    let day_count = (end_date - start_date).num_days();
    if day_count < 0 {
        return Ok(Vec::new());
    }

    (0..=day_count)
        .map(|offset| {
            let date = start_date + Duration::days(offset);
            local_start_of_day(date, &clock.tz)?;
            Ok(date.format("%Y-%m-%d").to_string())
        })
        .collect()
}
```

Add this helper to `usage.rs` near `build_daily_series`:

```rust
async fn chart_start_for_range(
    conn: &Connection,
    clock: &usage_time::UsageClock,
    range: &usage_time::UsageWindowRange,
) -> Result<Option<DateTime<Utc>>, ReadModelError> {
    if let Some(since) = range.since_utc.as_ref() {
        return Ok(Some(since.to_owned()));
    }

    first_usage_chart_start_utc(conn, clock).await
}

async fn first_usage_chart_start_utc(
    conn: &Connection,
    clock: &usage_time::UsageClock,
) -> Result<Option<DateTime<Utc>>, ReadModelError> {
    let coarse_until = (clock.now_utc + Duration::days(1)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT ts
         FROM usage_events
         WHERE ts <= ?1
         ORDER BY ts ASC",
    )?;
    let rows = stmt
        .query_map(params![coarse_until], |row| row.get::<_, String>(0))
        .await?;

    for row in rows {
        let ts = row?;
        let event_at = DateTime::parse_from_rfc3339(&ts)?.with_timezone(&Utc);
        if event_at > clock.now_utc {
            continue;
        }
        let local_date = event_at.with_timezone(&clock.tz).date_naive();
        return usage_time::local_date_start_utc(local_date, &clock.tz).map(Some);
    }

    Ok(None)
}
```

- [ ] **Step 4: Update existing tests for one selected window**

In `crates/right-dashboard/src/read_model/usage.rs`, update assertions that expect four windows. For `usage_overview_builds_windows_and_sources`, assert one selected default window:

```rust
assert_eq!(response.selected_range, "last_7_days");
assert_eq!(response.selected_window, "last_7_days");
assert_eq!(response.window.key, "last_7_days");
assert_eq!(
    response
        .windows
        .iter()
        .map(|window| window.key.as_str())
        .collect::<Vec<_>>(),
    vec!["last_7_days"]
);
```

Where tests currently read `today` from `response.windows`, either set `range: Some("today".to_owned())` in that test input, or read `response.window` if the selected range is the default. Use the narrower option that preserves the test's intent.

For `budget_skip_count_appears_in_window`, split the old all-window assertion into two calls:

```rust
let today_response = usage_overview(
    &conn,
    UsageOverviewInput {
        agent: "agent-b".to_owned(),
        generated_at: "2026-05-21T05:00:00Z".to_owned(),
        timezone: Some("UTC".to_owned()),
        range: Some("today".to_owned()),
    },
)
.await
.unwrap();
assert_eq!(today_response.window.budget_skip_count, 2);

let all_time_response = usage_overview(
    &conn,
    UsageOverviewInput {
        agent: "agent-b".to_owned(),
        generated_at: "2026-05-21T05:00:00Z".to_owned(),
        timezone: Some("UTC".to_owned()),
        range: Some("all_time".to_owned()),
    },
)
.await
.unwrap();
assert_eq!(all_time_response.window.budget_skip_count, 3);
```

- [ ] **Step 5: Run selected-range tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_range_tests
```

Expected: PASS.

- [ ] **Step 6: Run existing usage read-model tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard read_model::usage
```

Expected: PASS after updating one-window assumptions.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/usage.rs crates/right-dashboard/src/read_model/usage_time.rs crates/right-dashboard/src/read_model/usage_local_time_tests.rs crates/right-dashboard/src/read_model/usage_range_tests.rs
devenv shell -- git commit -m "feat(usage): select dashboard usage range server-side"
```

### Task 3: Backend Cron Job Breakdown

**Files:**
- Create: `crates/right-dashboard/src/read_model/usage_cron_jobs_tests.rs`
- Modify: `crates/right-dashboard/src/read_model/usage.rs`

- [ ] **Step 1: Write failing cron-job tests**

Create `crates/right-dashboard/src/read_model/usage_cron_jobs_tests.rs`:

```rust
use super::*;
use right_db::{open_connection, params};
use tempfile::tempdir;

async fn insert_usage(
    conn: &right_db::Connection,
    ts: &str,
    source: &str,
    job_name: Option<&str>,
    cost: f64,
    model: &str,
) {
    let model_json = format!(
        r#"{{"{model}":{{"costUSD":{cost},"inputTokens":10,"outputTokens":20,"cacheCreationInputTokens":5,"cacheReadInputTokens":40}}}}"#
    );
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (?1, ?2, 1, 0, ?3, ?4, ?5, 1, 10, 20, 5, 40, 1, 2, ?6, 'none')",
        params![ts, source, job_name, format!("{source}-{model}-{ts}"), cost, model_json],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn usage_overview_groups_cron_jobs_for_selected_range() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-02T08:00:00Z", "cron", Some("daily"), 0.10, "sonnet").await;
    insert_usage(&conn, "2026-06-03T08:00:00Z", "cron", Some("daily"), 0.40, "sonnet").await;
    insert_usage(&conn, "2026-06-04T08:00:00Z", "cron", Some("weekly"), 0.20, "opus").await;
    insert_usage(&conn, "2026-06-04T09:00:00Z", "reflection", Some("daily"), 9.99, "opus").await;
    insert_usage(&conn, "2026-06-04T10:00:00Z", "interactive", None, 8.88, "sonnet").await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("last_3_days".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        response.cron_jobs.iter().map(|job| job.job_name.as_str()).collect::<Vec<_>>(),
        vec!["daily", "weekly"]
    );
    let daily = &response.cron_jobs[0];
    assert!((daily.cost_usd - 0.50).abs() < 1e-9);
    assert_eq!(daily.invocations, 2);
    assert_eq!(daily.turns, 2);
    assert_eq!(daily.input_tokens, 20);
    assert_eq!(daily.output_tokens, 40);
    assert_eq!(daily.cache_creation_tokens, 10);
    assert_eq!(daily.cache_read_tokens, 80);
    assert_eq!(daily.web_search_requests, 2);
    assert_eq!(daily.web_fetch_requests, 4);
    assert_eq!(daily.per_model.len(), 1);
    assert_eq!(daily.per_model[0].model, "sonnet");
    assert!((daily.per_model[0].cost_usd - 0.50).abs() < 1e-9);
}

#[tokio::test]
async fn usage_overview_labels_null_cron_job_name_as_unknown() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).await.unwrap();
    insert_usage(&conn, "2026-06-04T08:00:00Z", "cron", None, 0.10, "sonnet").await;

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-06-04T12:00:00Z".to_owned(),
            timezone: Some("UTC".to_owned()),
            range: Some("today".to_owned()),
        },
    )
    .await
    .unwrap();

    assert_eq!(response.cron_jobs.len(), 1);
    assert_eq!(response.cron_jobs[0].job_name, "(unknown job)");
    assert!((response.cron_jobs[0].cost_usd - 0.10).abs() < 1e-9);
}
```

Register the test module at the bottom of `usage.rs`:

```rust
#[cfg(test)]
#[path = "usage_cron_jobs_tests.rs"]
mod usage_cron_jobs_tests;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_cron_jobs_tests
```

Expected: FAIL because `build_cron_jobs` still returns an empty vector.

- [ ] **Step 3: Replace the cron stub with real aggregation**

In `crates/right-dashboard/src/read_model/usage.rs`, replace the temporary `build_cron_jobs` stub with:

```rust
const UNKNOWN_CRON_JOB: &str = "(unknown job)";

struct CronJobAggregateRow {
    ts: String,
    job_name: Option<String>,
    cost: f64,
    turns: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    web_search_requests: i64,
    web_fetch_requests: i64,
    model_usage_json: String,
    api_key_source: String,
}

struct CronJobAccumulator {
    summary: UsageCronJobSummary,
    models: BTreeMap<String, UsageModelSummary>,
}

async fn build_cron_jobs(
    conn: &Connection,
    since: Option<&DateTime<Utc>>,
    until: &DateTime<Utc>,
) -> Result<Vec<UsageCronJobSummary>, ReadModelError> {
    let coarse_since = since.map(|since| (*since - Duration::days(1)).to_rfc3339());
    let coarse_until = (*until + Duration::days(1)).to_rfc3339();
    let rows = aggregate_cron_job_rows(conn, coarse_since.as_deref(), &coarse_until).await?;
    let mut jobs = BTreeMap::<String, CronJobAccumulator>::new();

    for row in rows {
        let event_at = DateTime::parse_from_rfc3339(&row.ts)?.with_timezone(&Utc);
        if since.is_some_and(|since| event_at < *since) || event_at > *until {
            continue;
        }

        let job_name = row.job_name.unwrap_or_else(|| UNKNOWN_CRON_JOB.to_owned());
        let entry = jobs.entry(job_name.clone()).or_insert_with(|| CronJobAccumulator {
            summary: UsageCronJobSummary {
                job_name: job_name.clone(),
                cost_usd: 0.0,
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
                per_model: Vec::new(),
            },
            models: BTreeMap::new(),
        });

        entry.summary.cost_usd += row.cost;
        if row.api_key_source == "none" {
            entry.summary.subscription_cost_usd += row.cost;
        } else {
            entry.summary.api_cost_usd += row.cost;
        }
        entry.summary.turns += row.turns.max(0) as u64;
        entry.summary.invocations += 1;
        entry.summary.input_tokens += row.input_tokens.max(0) as u64;
        entry.summary.output_tokens += row.output_tokens.max(0) as u64;
        entry.summary.cache_creation_tokens += row.cache_creation_tokens.max(0) as u64;
        entry.summary.cache_read_tokens += row.cache_read_tokens.max(0) as u64;
        entry.summary.web_search_requests += row.web_search_requests.max(0) as u64;
        entry.summary.web_fetch_requests += row.web_fetch_requests.max(0) as u64;
        aggregate_model_usage_for_window(&mut entry.models, "cron", &row.model_usage_json);
    }

    let mut rows = jobs
        .into_values()
        .map(|mut accumulator| {
            accumulator.summary.per_model = accumulator.models.into_values().collect::<Vec<_>>();
            sort_models(&mut accumulator.summary.per_model);
            accumulator.summary
        })
        .collect::<Vec<_>>();
    sort_cron_jobs(&mut rows);
    Ok(rows)
}

async fn aggregate_cron_job_rows(
    conn: &Connection,
    coarse_since: Option<&str>,
    coarse_until: &str,
) -> Result<Vec<CronJobAggregateRow>, ReadModelError> {
    if let Some(coarse_since) = coarse_since {
        let mut stmt = conn.prepare(
            "SELECT ts, job_name, total_cost_usd, num_turns, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, web_search_requests,
                    web_fetch_requests, model_usage_json, api_key_source
             FROM usage_events
             WHERE source = 'cron' AND ts >= ?1 AND ts <= ?2
             ORDER BY ts ASC",
        )?;
        return stmt
            .query_map(params![coarse_since, coarse_until], cron_job_aggregate_row)
            .await?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
    }

    let mut stmt = conn.prepare(
        "SELECT ts, job_name, total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
         FROM usage_events
         WHERE source = 'cron' AND ts <= ?1
         ORDER BY ts ASC",
    )?;
    stmt.query_map(params![coarse_until], cron_job_aggregate_row)
        .await?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn cron_job_aggregate_row(
    row: &right_db::row::Row<'_>,
) -> Result<CronJobAggregateRow, right_db::DbError> {
    Ok(CronJobAggregateRow {
        ts: row.get(0)?,
        job_name: row.get(1)?,
        cost: row.get(2)?,
        turns: row.get(3)?,
        input_tokens: row.get(4)?,
        output_tokens: row.get(5)?,
        cache_creation_tokens: row.get(6)?,
        cache_read_tokens: row.get(7)?,
        web_search_requests: row.get(8)?,
        web_fetch_requests: row.get(9)?,
        model_usage_json: row.get(10)?,
        api_key_source: row.get(11)?,
    })
}

fn sort_cron_jobs(rows: &mut [UsageCronJobSummary]) {
    rows.sort_by(|left, right| {
        right
            .cost_usd
            .partial_cmp(&left.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.job_name.cmp(&right.job_name))
    });
}
```

- [ ] **Step 4: Run cron-job tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_cron_jobs_tests
```

Expected: PASS.

- [ ] **Step 5: Run targeted dashboard read-model suite**

Run:

```bash
devenv shell -- cargo test -p right-dashboard read_model::usage
```

Expected: PASS.

- [ ] **Step 6: Run Rust review subagent if available**

If the executor environment has the requested Rust review skill/subagent, run `rust-dev:review-rust-code` against the Rust changes. Convert actionable findings into tracked action items, fix them one by one, and rerun:

```bash
devenv shell -- cargo test -p right-dashboard read_model::usage
```

Expected: PASS after fixes.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/read_model/usage.rs crates/right-dashboard/src/read_model/usage_cron_jobs_tests.rs
devenv shell -- git commit -m "feat(usage): aggregate cron jobs in dashboard usage"
```

### Task 4: Dashboard Route Query Support

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Write failing route test**

In `crates/bot/src/telegram/dashboard.rs`, add this test next to `usage_accepts_timezone_query_for_authorized_user`:

```rust
#[tokio::test]
async fn usage_accepts_range_query_for_authorized_user() {
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
            '2026-06-03T20:00:00Z', 'cron', 1, 0, 'daily', 's1',
            0.15, 1, 10, 20, 0, 0, 0, 0,
            '{\"sonnet\":{\"costUSD\":0.15}}', 'none'
         )",
        [],
    )
    .await
    .unwrap();

    let (status, body) = get_json(
        "/dashboard/alpha/api/v1/usage?timezone=Asia%2FDubai&range=last_3_days",
        Some(signed_init_data(42)),
        temp.path().to_path_buf(),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["selected_range"], "last_3_days");
    assert_eq!(body["window"]["key"], "last_3_days");
    assert_eq!(body["cron_jobs"][0]["job_name"], "daily");
}
```

Update the existing `usage_returns_structured_windows_for_authorized_user` expectations:

```rust
assert_eq!(body["selected_range"], "last_7_days");
assert_eq!(body["selected_window"], "last_7_days");
assert_eq!(body["window"]["key"], "last_7_days");
assert!(body["windows"].is_array());
assert_eq!(body["windows"].as_array().unwrap().len(), 1);
assert_eq!(body["windows"][0]["key"], "last_7_days");
assert!(body.get("cron_jobs").is_some(), "usage must expose cron_jobs");
```

- [ ] **Step 2: Run route test to verify it fails**

Run:

```bash
devenv shell -- cargo test -p bot usage_accepts_range_query_for_authorized_user
```

Expected: FAIL because `UsageOverviewQuery` does not yet deserialize or pass `range`.

- [ ] **Step 3: Pass range through the route**

Modify `UsageOverviewQuery` in `crates/bot/src/telegram/dashboard.rs`:

```rust
#[derive(Debug, Deserialize)]
struct UsageOverviewQuery {
    timezone: Option<String>,
    range: Option<String>,
}
```

Modify the `UsageOverviewInput` construction:

```rust
let input = UsageOverviewInput {
    agent: state.agent_name.clone(),
    generated_at: chrono::Utc::now().to_rfc3339(),
    timezone: query.timezone,
    range: query.range,
};
```

- [ ] **Step 4: Run route tests**

Run:

```bash
devenv shell -- cargo test -p bot usage_accepts_range_query_for_authorized_user usage_returns_structured_windows_for_authorized_user usage_accepts_timezone_query_for_authorized_user
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(usage): accept dashboard usage range query"
```

### Task 5: Frontend API Types and Range Helpers

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Create: `crates/right-dashboard/frontend/src/views/usageRanges.ts`
- Modify: `crates/right-dashboard/frontend/src/api.ts`
- Modify: `crates/right-dashboard/frontend/src/api.test.ts`

- [ ] **Step 1: Write failing API tests**

Update `crates/right-dashboard/frontend/src/api.test.ts`.

Change the payload:

```ts
function usagePayload() {
  return {
    agent: 'right',
    generated_at: '2026-06-04T12:00:00Z',
    timezone: 'Asia/Dubai',
    selected_range: 'last_7_days',
    selected_window: 'last_7_days',
    window: {
      key: 'last_7_days',
      label: 'Last 7 days',
      range_start: '2026-05-29T00:00:00+04:00',
      range_end: '2026-06-04T16:00:00+04:00',
      range_label: 'Asia/Dubai · May 29 00:00-Jun 4 16:00',
      sources: [],
      total_cost_usd: 0,
      subscription_cost_usd: 0,
      api_cost_usd: 0,
      turns: 0,
      invocations: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_creation_tokens: 0,
      cache_read_tokens: 0,
      web_search_requests: 0,
      web_fetch_requests: 0,
      per_model: [],
      budget_skip_count: 0,
    },
    windows: [],
    daily_series: [],
    source_series: [],
    cron_jobs: [],
    warnings: [],
  }
}
```

Update the existing URL assertion:

```ts
expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=Asia%2FDubai&range=last_7_days')
```

Add a selected range test:

```ts
it('sends an explicit usage range when provided', async () => {
  const fetchMock = vi.fn(async (_path: string, _options?: RequestInit) => new Response(JSON.stringify(usagePayload()), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  }))
  vi.stubGlobal('fetch', fetchMock)
  vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })

  await usageOverview({ timezone: 'UTC', range: 'last_3_days' })

  expect(fetchMock).toHaveBeenCalledOnce()
  expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=UTC&range=last_3_days')
})
```

- [ ] **Step 2: Run API tests to verify failure**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/api.test.ts
```

Expected: FAIL because `usageOverview` does not accept an options object or send `range`.

- [ ] **Step 3: Add range types**

Modify `crates/right-dashboard/frontend/src/types.ts`.

Add:

```ts
export type UsageRange = 'today' | 'last_3_days' | 'last_7_days' | 'last_30_days' | 'all_time'
```

Modify `UsageOverviewResponse`:

```ts
export interface UsageOverviewResponse {
  agent: string
  generated_at: string
  timezone: string
  selected_range: UsageRange
  window: UsageWindow
  windows: UsageWindow[]
  selected_window: string
  daily_series: UsageDailyPoint[]
  source_series: UsageSourceSeries[]
  cron_jobs: UsageCronJobSummary[]
  warnings: DashboardDataWarning[]
}
```

Add:

```ts
export interface UsageCronJobSummary {
  job_name: string
  cost_usd: number
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
}
```

- [ ] **Step 4: Add range helpers**

Create `crates/right-dashboard/frontend/src/views/usageRanges.ts`:

```ts
import type { UsageRange } from '../types'

export const DEFAULT_USAGE_RANGE: UsageRange = 'last_7_days'

export const USAGE_RANGE_OPTIONS: Array<{ key: UsageRange, label: string }> = [
  { key: 'today', label: 'Today' },
  { key: 'last_3_days', label: '3 days' },
  { key: 'last_7_days', label: '7 days' },
  { key: 'last_30_days', label: '30 days' },
  { key: 'all_time', label: 'All time' },
]

export function isUsageRange(value: string | null | undefined): value is UsageRange {
  return USAGE_RANGE_OPTIONS.some((option) => option.key === value)
}
```

- [ ] **Step 5: Update API request code**

Modify imports in `crates/right-dashboard/frontend/src/api.ts`:

```ts
import type {
  ApiErrorBody,
  DashboardOverviewResponse,
  DoctorResponse,
  IdentityFileResponse,
  BootstrapResponse,
  IdentityResponse,
  LearningOverviewResponse,
  McpAddRequest,
  McpDetectRequest,
  McpDetectResponse,
  McpHeaderInput,
  McpHeadersRequest,
  McpMutationResponse,
  McpOAuthStartResponse,
  McpOAuthStatusResponse,
  McpServersResponse,
  OverviewResponse,
  PinSkillRequest,
  PinSkillResponse,
  ProviderCreateBody,
  ProviderGenericBody,
  ProviderProfileView,
  ProviderView,
  RunDetailResponse,
  SkillDetailResponse,
  SkillsResponse,
  SandboxStatsResponse,
  UsageOverviewResponse,
  UsageRange,
} from './types'
import { DEFAULT_USAGE_RANGE } from './views/usageRanges'
```

Replace `usageOverview`:

```ts
export interface UsageOverviewOptions {
  timezone?: string
  range?: UsageRange | string
}

export function usageOverview(options: UsageOverviewOptions = {}): Promise<UsageOverviewResponse> {
  const params = new URLSearchParams({
    timezone: options.timezone ?? browserUsageTimezone(),
    range: options.range ?? DEFAULT_USAGE_RANGE,
  })
  return requestJson<UsageOverviewResponse>(`api/v1/usage?${params.toString()}`)
}
```

- [ ] **Step 6: Run frontend API tests**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/api.test.ts
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/views/usageRanges.ts crates/right-dashboard/frontend/src/api.ts crates/right-dashboard/frontend/src/api.test.ts
devenv shell -- git commit -m "feat(usage): add frontend usage range contract"
```

### Task 6: Frontend Range UI and Single Window Rendering

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/UsageContainer.vue`
- Create: `crates/right-dashboard/frontend/src/views/UsageContainer.test.ts`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.test.ts`
- Modify: `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`

- [ ] **Step 1: Write failing SSR tests**

Create `crates/right-dashboard/frontend/src/views/UsageContainer.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageContainer from './UsageContainer.vue'

async function render() {
  const app = createSSRApp({ render: () => h(UsageContainer) })
  return renderToString(app)
}

describe('UsageContainer range selection', () => {
  it('renders last 7 days as the default active range', async () => {
    const html = await render()
    expect(html).toContain('7 days')
    expect(html).toContain('active')
    expect(html).toContain('No usage data')
  })
})
```

Update `crates/right-dashboard/frontend/src/views/UsageView.test.ts` stubs:

```ts
import type {
  UsageCronJobSummary,
  UsageDailyPoint,
  UsageOverviewResponse,
  UsageSourceSummary,
  UsageWindow,
} from '../types'
```

Add a cron job stub:

```ts
function cronJobStub(overrides: Partial<UsageCronJobSummary> = {}): UsageCronJobSummary {
  const base: UsageCronJobSummary = {
    job_name: 'daily',
    cost_usd: 2.5,
    subscription_cost_usd: 2.5,
    api_cost_usd: 0,
    turns: 2,
    invocations: 2,
    input_tokens: 200,
    output_tokens: 100,
    cache_creation_tokens: 50,
    cache_read_tokens: 300,
    web_search_requests: 0,
    web_fetch_requests: 0,
    per_model: [],
  }
  return Object.assign(base, overrides)
}
```

Update `usageStub`:

```ts
function usageStub(overrides: Partial<UsageOverviewResponse> = {}): UsageOverviewResponse {
  const window = windowStub()
  const base: UsageOverviewResponse = {
    agent: 'test-agent',
    generated_at: '2026-01-01T00:00:00Z',
    timezone: 'Asia/Dubai',
    selected_range: 'last_7_days',
    window,
    windows: [window],
    selected_window: 'last_7_days',
    daily_series: [],
    source_series: [],
    cron_jobs: [],
    warnings: [],
  }
  return Object.assign(base, overrides)
}
```

Add tests:

```ts
describe('UsageView selected range rendering', () => {
  it('renders range segments and only the selected window', async () => {
    const selected = windowStub({ key: 'last_7_days', label: 'Last 7 days' })
    const stale = windowStub({ key: 'today', label: 'Today', total_cost_usd: 99 })
    const html = await render({
      usage: usageStub({ window: selected, windows: [selected, stale], selected_range: 'last_7_days' }),
      selectedRange: 'last_7_days',
      loading: false,
      error: null,
    })

    expect(html).toContain('Today')
    expect(html).toContain('3 days')
    expect(html).toContain('7 days')
    expect(html).toContain('30 days')
    expect(html).toContain('All time')
    expect(html).toContain('Last 7 days')
    expect(html).not.toContain('$99.00')
  })

  it('renders selected cron job rows with token lines', async () => {
    const html = await render({
      usage: usageStub({ cron_jobs: [cronJobStub()] }),
      selectedRange: 'last_7_days',
      loading: false,
      error: null,
    })

    expect(html).toContain('Cron jobs')
    expect(html).toContain('daily')
    expect(html).toContain('$2.50')
    expect(html).toContain('token-line')
  })
})
```

- [ ] **Step 2: Run UsageView tests to verify failure**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/views/UsageContainer.test.ts src/views/UsageView.test.ts
```

Expected: FAIL because props, range segments, `window`, and cron-job rendering are not implemented.

- [ ] **Step 3: Update `UsageContainer.vue`**

Replace `crates/right-dashboard/frontend/src/views/UsageContainer.vue` with:

```vue
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { usageOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import type { UsageRange } from '../types'
import { DEFAULT_USAGE_RANGE, isUsageRange } from './usageRanges'
import UsageView from './UsageView.vue'

const requestedRange = ref<UsageRange>(DEFAULT_USAGE_RANGE)

const { data, loading, error, refresh } = useLiveResource(
  () => usageOverview({ range: requestedRange.value }),
  { key: 'usage' },
)

const effectiveRange = computed<UsageRange>(() => {
  const responseRange = data.value?.selected_range
  return isUsageRange(responseRange) ? responseRange : requestedRange.value
})

watch(requestedRange, () => {
  void refresh()
})

function selectRange(range: UsageRange): void {
  if (range !== requestedRange.value) {
    requestedRange.value = range
  }
}
</script>

<template>
  <UsageView
    :usage="data"
    :selected-range="effectiveRange"
    :loading="loading"
    :error="error"
    @select-range="selectRange"
  />
</template>
```

- [ ] **Step 4: Update chart title prop**

Modify `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`.

Add prop:

```ts
const props = defineProps<{
  points: UsageDailyPoint[]
  selectedDate: string | null
  title?: string
}>()
```

Change the header:

```vue
<h2>{{ title ?? 'Spend' }}</h2>
```

- [ ] **Step 5: Update `UsageView.vue`**

Modify imports and props:

```vue
<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import UsageBreakdown from '../components/charts/UsageBreakdown.vue'
import UsageSpendChart from '../components/charts/UsageSpendChart.vue'
import AsyncState from '../components/AsyncState.vue'
import TokenLine from '../components/charts/TokenLine.vue'
import TokenLegend from '../components/charts/TokenLegend.vue'
import { money } from '../format'
import type { UsageCronJobSummary, UsageOverviewResponse, UsageRange, UsageWindow } from '../types'
import { USAGE_RANGE_OPTIONS } from './usageRanges'
import { selectedDayRangeLabel } from './usageDayRange'

const props = defineProps<{
  usage: UsageOverviewResponse | null
  selectedRange: UsageRange
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectRange: [range: UsageRange]
}>()
```

Add selected window helpers:

```ts
const selectedWindow = computed<UsageWindow | null>(() => props.usage?.window ?? props.usage?.windows?.[0] ?? null)

function windowRows(window: UsageWindow | null | undefined) {
  return window?.sources ?? []
}

function cronRows(rows: UsageCronJobSummary[] | null | undefined) {
  return rows ?? []
}
```

Keep the existing selected day logic, but make it read from selected range data:

```ts
watchEffect(() => {
  const points = props.usage?.daily_series ?? []
  if (points.length === 0) {
    selectedDate.value = null
    return
  }

  if (selectedDate.value === null || !points.some((point) => point.date === selectedDate.value)) {
    selectedDate.value = points[points.length - 1].date
  }
})
```

Replace the template with this shape:

```vue
<template>
  <nav class="segmented" aria-label="Usage range">
    <button
      v-for="range in USAGE_RANGE_OPTIONS"
      :key="range.key"
      class="segment-button"
      :class="{ active: range.key === selectedRange }"
      type="button"
      @click="emit('selectRange', range.key)"
    >
      {{ range.label }}
    </button>
  </nav>

  <AsyncState :loading="loading" :error="error" :empty="usage === null && !loading" empty-text="No usage data">
    <section v-if="usage?.warnings.length" class="notice">
      <strong>Partial data</strong>
      <span v-for="warning in usage.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
        {{ warning.message }}
      </span>
    </section>

    <TokenLegend :sticky="false" />

    <section class="two-column wide-main">
      <UsageSpendChart
        :points="usage?.daily_series ?? []"
        :selected-date="selectedDate"
        :title="selectedWindow?.label ?? 'Usage'"
        @select-date="selectedDate = $event"
      />
      <UsageBreakdown :point="selectedPoint" :range-label="selectedRangeLabel" />
    </section>

    <section class="list-stack">
      <article v-if="selectedWindow" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">{{ selectedWindow.key }}</p>
            <h2>{{ selectedWindow.label }}</h2>
            <p class="muted-line">{{ selectedWindow.range_label }}</p>
          </div>
          <strong>{{ money(selectedWindow.total_cost_usd) }}</strong>
        </header>
        <p v-if="selectedWindow.budget_skip_count > 0" class="muted-line">
          Budget-blocked learning attempts: {{ selectedWindow.budget_skip_count }}
        </p>

        <div class="model-grid">
          <div v-for="source in windowRows(selectedWindow)" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <TokenLine :tokens="source" compact />
          </div>
        </div>

        <section class="text-block">
          <h3>Cron jobs</h3>
          <div class="row-list">
            <div v-for="job in cronRows(usage?.cron_jobs)" :key="job.job_name" class="usage-source">
              <div class="model-row">
                <span>{{ job.job_name }}</span>
                <strong>{{ money(job.cost_usd) }}</strong>
              </div>
              <TokenLine :tokens="job" compact />
            </div>
            <p v-if="cronRows(usage?.cron_jobs).length === 0" class="muted-line">No cron job spend</p>
          </div>
        </section>
      </article>

      <article v-else class="empty-panel">No usage data for period</article>
    </section>

    <TokenLegend />
  </AsyncState>
</template>
```

- [ ] **Step 6: Run frontend Usage tests**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/views/UsageContainer.test.ts src/views/UsageView.test.ts
```

Expected: PASS.

- [ ] **Step 7: Run frontend typecheck**

Run:

```bash
devenv shell -- pnpm --dir crates/right-dashboard/frontend typecheck
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/views/UsageContainer.vue crates/right-dashboard/frontend/src/views/UsageContainer.test.ts crates/right-dashboard/frontend/src/views/UsageView.vue crates/right-dashboard/frontend/src/views/UsageView.test.ts crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue
devenv shell -- git commit -m "feat(usage): render selected usage range"
```

### Task 7: Architecture Doc and Full Verification

**Files:**
- Modify: `docs/architecture/modules.md`

- [ ] **Step 1: Update architecture satellite doc**

In `docs/architecture/modules.md`, replace the `read_model/usage.rs` bullet with:

```markdown
- `read_model/usage.rs` - usage/cost projections over `usage_events`, including selected-range totals, source splits, cron-job breakdowns, and model summaries. Usage read models accept a viewer timezone and selected Usage range, bucket Usage-tab windows by that local calendar, then convert bounds back to UTC for storage filtering.
```

- [ ] **Step 2: Run targeted backend and frontend checks**

Run:

```bash
devenv shell -- cargo test -p right-dashboard read_model::usage
devenv shell -- cargo test -p bot usage_accepts_range_query_for_authorized_user usage_returns_structured_windows_for_authorized_user usage_accepts_timezone_query_for_authorized_user
devenv shell -- pnpm --dir crates/right-dashboard/frontend test -- src/api.test.ts src/views/UsageContainer.test.ts src/views/UsageView.test.ts src/components/charts/UsageBreakdown.test.ts
devenv shell -- pnpm --dir crates/right-dashboard/frontend typecheck
```

Expected: all PASS.

- [ ] **Step 3: Run final mandatory workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If this fails for a pre-existing unrelated issue, record the exact failing test and why targeted checks still prove this feature. Do not claim full completion without stating the failure.

- [ ] **Step 4: Check worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: only intended feature files modified plus any pre-existing unrelated untracked files.

- [ ] **Step 5: Commit docs and final fixes**

Run:

```bash
devenv shell -- git add docs/architecture/modules.md
devenv shell -- git commit -m "docs(usage): document selected usage range projections"
```

If Task 7 includes code fixes from verification, include those files in the same commit only if they are directly tied to the verification failure:

```bash
devenv shell -- git add docs/architecture/modules.md <directly-related-file>
devenv shell -- git commit -m "fix(usage): stabilize selected range verification"
```

## Self-Review Checklist

- Spec coverage:
  - Range modes and default are covered by Tasks 1, 2, 5, and 6.
  - Backend-selected range is covered by Tasks 2 and 4.
  - One selected period UI is covered by Task 6.
  - Cron job breakdown, `source = 'cron'` only, null job labels, and sorting are covered by Task 3.
  - Timezone-local calendar semantics are covered by Task 2.
  - Architecture doc cite-on-touch is covered by Task 7.
- Placeholder scan: the plan was searched for forbidden placeholder markers and any matches were rewritten.
- Type consistency:
  - Rust response fields are `selected_range`, `window`, `windows`, `selected_window`, `daily_series`, `source_series`, `cron_jobs`.
  - TypeScript response fields match the Rust DTO names.
  - Range keys match across Rust, TypeScript, API query strings, and tests.
