# Dashboard Visual Analytics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build chart-led dashboard overview, usage, and learning views backed by explicit read-only visual analytics DTOs.

**Architecture:** `right-dashboard` owns the new DTOs and SQLite read-model projections. `right-bot` keeps the existing route/auth boundary and benefits automatically because the existing handlers serialize the expanded response types. The Vue Mini App uses ECharts/Vue-ECharts with on-demand imports and chart components that expose hover/select details without chart-driven route navigation.

**Tech Stack:** Rust 2024, `right-dashboard`, `right-bot`, SQLite/rusqlite, Vue 3, Vite, TypeScript, `echarts`, `vue-echarts`.

---

## File Structure

- Modify `crates/right-dashboard/src/api_types.rs`
  Add warning, overview signal, cost/learning river, usage daily series, and learning-flow DTOs. Extend `DashboardOverviewResponse`, `UsageOverviewResponse`, and `LearningOverviewResponse`.

- Modify `crates/right-dashboard/src/read_model/usage.rs`
  Build last-30-day daily usage series and source series from `usage_events`.

- Modify `crates/right-dashboard/src/read_model/dashboard_overview.rs`
  Build the signal-led overview timeline and cost/learning river from usage, learning events, curator state, async runs, and injected sandbox status.

- Modify `crates/right-dashboard/src/read_model/learning.rs`
  Build learning-flow nodes/edges and recent learning signals from current learning tables and `skill_learning_events`.

- Modify `crates/bot/src/telegram/dashboard.rs`
  Add/adjust handler tests only if existing tests assert exact JSON shapes. Route code should not need structural changes.

- Modify `crates/right-dashboard/frontend/package.json`
  Add `echarts` and `vue-echarts`.

- Modify `crates/right-dashboard/frontend/package-lock.json`
  Regenerate by running `npm install` in `crates/right-dashboard/frontend`.

- Modify `crates/right-dashboard/frontend/src/types.ts`
  Mirror all new DTO fields.

- Create `crates/right-dashboard/frontend/src/charts.ts`
  Centralize on-demand ECharts module registration.

- Create `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue`
  Render signal-led timeline rows with stable empty/loading layout.

- Create `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue`
  Render the cost/learning river using ECharts `themeRiver`.

- Create `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`
  Render stacked daily spend by source and emit selected date.

- Create `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
  Render selected-day source/model/token/API split details.

- Create `crates/right-dashboard/frontend/src/components/charts/LearningFlowChart.vue`
  Render learning-flow nodes/edges using ECharts `sankey`.

- Create `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue`
  Render recent learning-signal detail beside the flow chart.

- Modify `crates/right-dashboard/frontend/src/views/OverviewView.vue`
  Make the Overview tab timeline-first with cost/learning river as secondary visual.

- Modify `crates/right-dashboard/frontend/src/views/UsageView.vue`
  Make Usage spend-over-time first and keep source/model rows secondary.

- Modify `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`
  Make Knowledge/Reports learning-flow first and keep reports as secondary inspection.

- Modify `crates/right-dashboard/frontend/src/App.vue`
  Add any shared chart layout CSS needed by the new components.

- Modify `docs/architecture/modules.md`
  Update the `right-dashboard` module notes if the final implementation adds new chart components or read-model responsibilities not already described.

- Modify `ARCHITECTURE.md`
  Update only if implementation changes the dashboard API contract in a way that is architecturally load-bearing beyond existing read-only dashboard ownership.

## Task 0: Baseline Checks

**Files:**
- Read: `docs/superpowers/specs/2026-05-23-dashboard-visual-analytics-design.md`
- Read: `docs/architecture/modules.md`
- Read: `ARCHITECTURE.md`

- [ ] **Step 1: Re-read the approved spec**

Run:

```bash
devenv shell -- sed -n '1,430p' docs/superpowers/specs/2026-05-23-dashboard-visual-analytics-design.md
```

Expected: the spec describes timeline-first Overview, spend-over-time Usage, and learning-flow Knowledge.

- [ ] **Step 2: Re-read dashboard architecture notes**

Run:

```bash
devenv shell -- sed -n '60,105p' docs/architecture/modules.md
devenv shell -- sed -n '1,70p' ARCHITECTURE.md
```

Expected: `right-dashboard` owns DTOs/read models/static assets and `right-bot` owns route mounting/auth.

- [ ] **Step 3: Run targeted backend baseline**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
```

Expected: PASS. Record any pre-existing failures before editing.

- [ ] **Step 4: Run targeted frontend baseline**

Run:

```bash
cd crates/right-dashboard/frontend
npm run build
```

Expected: PASS. Record any pre-existing failures before editing.

## Task 1: Visual Analytics DTOs

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`

- [ ] **Step 1: Write failing serialization tests**

Append these tests inside `mod dashboard_v2_tests` and `mod learning_tests` in `crates/right-dashboard/src/api_types.rs`.

```rust
// crates/right-dashboard/src/api_types.rs

#[test]
fn dashboard_visual_overview_serializes_expected_shape() {
    let response = DashboardOverviewResponse {
        agent: "alpha".to_owned(),
        generated_at: "2026-05-23T10:00:00Z".to_owned(),
        active_runs: 1,
        recent_failures: 1,
        today_cost_usd: 1.25,
        learning_candidates_24h: 2,
        doctor: OverviewDoctorStatus {
            state: "not_loaded".to_owned(),
            pass_count: 0,
            warn_count: 0,
            fail_count: 0,
            generated_at: None,
        },
        sandbox: OverviewSandboxStatus {
            state: "configured".to_owned(),
            detail: Some("sandbox alpha".to_owned()),
        },
        signals: vec![DashboardSignal {
            id: "learning:rightx-debug:2026-05-23T09:00:00Z".to_owned(),
            kind: "learning_outcome".to_owned(),
            severity: "info".to_owned(),
            occurred_at: "2026-05-23T09:00:00Z".to_owned(),
            title: "Skill created".to_owned(),
            detail: Some("rightx-debug".to_owned()),
            source: Some("learning_probe_writer".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: Some("rightx-debug".to_owned()),
            related_report_id: None,
        }],
        cost_learning_river: CostLearningRiver {
            window: "last_30_days".to_owned(),
            points: vec![CostLearningPoint {
                bucket: "2026-05-23".to_owned(),
                total_cost_usd: 1.25,
                sources: vec![UsageSourcePoint {
                    source: "interactive".to_owned(),
                    cost_usd: 1.25,
                    subscription_cost_usd: 1.25,
                    api_cost_usd: 0.0,
                    turns: 1,
                    invocations: 1,
                }],
            }],
            series: vec![CostLearningSeries {
                source: "interactive".to_owned(),
                points: vec![CostSeriesPoint {
                    bucket: "2026-05-23".to_owned(),
                    cost_usd: 1.25,
                }],
            }],
            markers: vec![LearningMarker {
                id: "marker:rightx-debug".to_owned(),
                occurred_at: "2026-05-23T09:00:00Z".to_owned(),
                kind: "skill_created".to_owned(),
                label: "rightx-debug".to_owned(),
                severity: "info".to_owned(),
                skill_name: Some("rightx-debug".to_owned()),
                source: Some("learning_probe_writer".to_owned()),
                cost_usd: None,
            }],
        },
        warnings: vec![DashboardDataWarning {
            source: "curator_state".to_owned(),
            kind: "unavailable".to_owned(),
            message: "curator state row is absent".to_owned(),
        }],
    };

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["signals"][0]["kind"], "learning_outcome");
    assert_eq!(value["cost_learning_river"]["points"][0]["bucket"], "2026-05-23");
    assert_eq!(value["warnings"][0]["kind"], "unavailable");
}

#[test]
fn usage_visual_series_serializes_expected_shape() {
    let response = UsageOverviewResponse {
        agent: "alpha".to_owned(),
        generated_at: "2026-05-23T10:00:00Z".to_owned(),
        windows: vec![],
        selected_window: "last_30_days".to_owned(),
        daily_series: vec![UsageDailyPoint {
            date: "2026-05-23".to_owned(),
            total_cost_usd: 1.25,
            subscription_cost_usd: 1.00,
            api_cost_usd: 0.25,
            turns: 2,
            invocations: 2,
            input_tokens: 10,
            output_tokens: 20,
            cache_creation_tokens: 5,
            cache_read_tokens: 40,
            web_search_requests: 1,
            web_fetch_requests: 2,
            sources: vec![UsageSourcePoint {
                source: "interactive".to_owned(),
                cost_usd: 1.25,
                subscription_cost_usd: 1.00,
                api_cost_usd: 0.25,
                turns: 2,
                invocations: 2,
            }],
            models: vec![UsageModelSummary {
                model: "sonnet".to_owned(),
                cost_usd: 1.25,
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_tokens: 5,
                cache_read_tokens: 40,
            }],
        }],
        source_series: vec![UsageSourceSeries {
            source: "interactive".to_owned(),
            points: vec![CostSeriesPoint {
                bucket: "2026-05-23".to_owned(),
                cost_usd: 1.25,
            }],
        }],
        warnings: vec![],
    };

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["selected_window"], "last_30_days");
    assert_eq!(value["daily_series"][0]["sources"][0]["source"], "interactive");
}
```

Append this test inside `mod learning_tests`:

```rust
// crates/right-dashboard/src/api_types.rs

#[test]
fn learning_flow_serializes_expected_shape() {
    let response = LearningOverviewResponse {
        agent: "right".to_owned(),
        generated_at: "2026-05-23T10:00:00Z".to_owned(),
        refresh_interval_secs: 5,
        capabilities: LearningCapabilities {
            learning_metrics: true,
            learning_evidence_snippets: true,
            learning_commands: false,
        },
        funnel: LearningFunnel {
            signals_accepted_24h: 1,
            episodes_pending_24h: 0,
            episodes_selecting_24h: 0,
            episodes_selected_24h: 0,
            episodes_reviewing_24h: 0,
            episodes_reviewed_24h: 0,
            episodes_no_episode_24h: 0,
            episodes_insufficient_context_24h: 0,
            episodes_failed_24h: 0,
            reports_total_24h: 0,
            create_candidates_24h: 0,
            update_candidates_24h: 0,
            nothing_to_learn_24h: 0,
            failed_reviews_24h: 0,
            foreground_created_or_updated_7d: 1,
        },
        quality: LearningQuality {
            candidate_rate: None,
            nothing_to_learn_rate: None,
            create_count_24h: 0,
            update_count_24h: 0,
            high_confidence_count_24h: 0,
            medium_confidence_count_24h: 0,
            low_confidence_count_24h: 0,
            failed_count_24h: 0,
        },
        health: LearningHealth {
            review_running: false,
            daily_review_count: 0,
            daily_limit: 12,
            creation_review_interval: 15,
            tool_iters_since_review: 0,
            turns_since_review: 0,
            skill_issue_hints_since_review: 0,
            last_review_status: None,
            last_review_at: None,
            possibly_stuck: false,
        },
        lifecycle: LearningLifecycle {
            created_7d: 1,
            updated_7d: 0,
            failed_or_aborted_7d: 0,
            recent_successful_events: vec![],
            candidate_skill_names_7d: vec![],
        },
        recent_reports: vec![],
        flow_nodes: vec![LearningFlowNode {
            id: "writer_created".to_owned(),
            label: "Created".to_owned(),
            kind: "writer".to_owned(),
            count: 1,
            severity: "info".to_owned(),
        }],
        flow_edges: vec![LearningFlowEdge {
            source: "prefilter_create".to_owned(),
            target: "writer_created".to_owned(),
            count: 1,
        }],
        recent_learning_signals: vec![LearningSignalPoint {
            id: "learn:rightx-debug".to_owned(),
            occurred_at: "2026-05-23T09:00:00Z".to_owned(),
            kind: "skill_created".to_owned(),
            label: "rightx-debug".to_owned(),
            severity: "info".to_owned(),
            skill_name: Some("rightx-debug".to_owned()),
            count: 1,
        }],
        warnings: vec![],
    };

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["flow_nodes"][0]["id"], "writer_created");
    assert_eq!(value["flow_edges"][0]["count"], 1);
    assert_eq!(value["recent_learning_signals"][0]["kind"], "skill_created");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_visual_overview_serializes_expected_shape usage_visual_series_serializes_expected_shape learning_flow_serializes_expected_shape
```

Expected: FAIL with missing types/fields such as `DashboardSignal`, `CostLearningRiver`, `UsageDailyPoint`, and `flow_nodes`.

- [ ] **Step 3: Add DTO structs and response fields**

Add these structs near the related existing DTOs in `crates/right-dashboard/src/api_types.rs`:

```rust
// crates/right-dashboard/src/api_types.rs

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DashboardDataWarning {
    pub source: String,
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardSignal {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub occurred_at: String,
    pub title: String,
    pub detail: Option<String>,
    pub source: Option<String>,
    pub cost_usd: Option<f64>,
    pub related_run_id: Option<String>,
    pub related_skill_name: Option<String>,
    pub related_report_id: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningRiver {
    pub window: String,
    pub points: Vec<CostLearningPoint>,
    pub series: Vec<CostLearningSeries>,
    pub markers: Vec<LearningMarker>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningPoint {
    pub bucket: String,
    pub total_cost_usd: f64,
    pub sources: Vec<UsageSourcePoint>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningSeries {
    pub source: String,
    pub points: Vec<CostSeriesPoint>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostSeriesPoint {
    pub bucket: String,
    pub cost_usd: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningMarker {
    pub id: String,
    pub occurred_at: String,
    pub kind: String,
    pub label: String,
    pub severity: String,
    pub skill_name: Option<String>,
    pub source: Option<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageDailyPoint {
    pub date: String,
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
    pub sources: Vec<UsageSourcePoint>,
    pub models: Vec<UsageModelSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourcePoint {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourceSeries {
    pub source: String,
    pub points: Vec<CostSeriesPoint>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFlowNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub count: i64,
    pub severity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFlowEdge {
    pub source: String,
    pub target: String,
    pub count: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningSignalPoint {
    pub id: String,
    pub occurred_at: String,
    pub kind: String,
    pub label: String,
    pub severity: String,
    pub skill_name: Option<String>,
    pub count: i64,
}
```

Extend existing response structs:

```rust
// crates/right-dashboard/src/api_types.rs

pub struct DashboardOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub active_runs: i64,
    pub recent_failures: i64,
    pub today_cost_usd: f64,
    pub learning_candidates_24h: i64,
    pub doctor: OverviewDoctorStatus,
    pub sandbox: OverviewSandboxStatus,
    pub signals: Vec<DashboardSignal>,
    pub cost_learning_river: CostLearningRiver,
    pub warnings: Vec<DashboardDataWarning>,
}

pub struct UsageOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub windows: Vec<UsageWindow>,
    pub selected_window: String,
    pub daily_series: Vec<UsageDailyPoint>,
    pub source_series: Vec<UsageSourceSeries>,
    pub warnings: Vec<DashboardDataWarning>,
}

pub struct LearningOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub capabilities: LearningCapabilities,
    pub funnel: LearningFunnel,
    pub quality: LearningQuality,
    pub health: LearningHealth,
    pub lifecycle: LearningLifecycle,
    pub recent_reports: Vec<LearningReportSummary>,
    pub flow_nodes: Vec<LearningFlowNode>,
    pub flow_edges: Vec<LearningFlowEdge>,
    pub recent_learning_signals: Vec<LearningSignalPoint>,
    pub warnings: Vec<DashboardDataWarning>,
}
```

Update existing tests in `api_types.rs` that construct these response structs by adding empty `signals`, empty/default `cost_learning_river`, `selected_window: "last_30_days"`, empty `daily_series`, empty `source_series`, empty `flow_nodes`, empty `flow_edges`, empty `recent_learning_signals`, and empty `warnings`.

- [ ] **Step 4: Run DTO tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard api_types
```

Expected: PASS.

- [ ] **Step 5: Commit DTOs**

```bash
git add crates/right-dashboard/src/api_types.rs
git commit -m "feat(dashboard): add visual analytics DTOs"
```

## Task 2: Usage Spend Series Read Model

**Files:**
- Modify: `crates/right-dashboard/src/read_model/usage.rs`

- [ ] **Step 1: Write failing usage aggregation tests**

Append these tests in `crates/right-dashboard/src/read_model/usage.rs` tests module:

```rust
// crates/right-dashboard/src/read_model/usage.rs

#[test]
fn usage_overview_builds_daily_series_for_last_30_days() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    insert_usage(&conn, "2026-05-01T08:00:00Z", "interactive", 0.10, "sonnet");
    insert_usage(&conn, "2026-05-01T09:00:00Z", "cron", 0.20, "opus");
    insert_usage(&conn, "2026-04-01T08:00:00Z", "interactive", 9.99, "old");

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-23T12:00:00Z".to_owned(),
        },
    )
    .unwrap();

    assert_eq!(response.selected_window, "last_30_days");
    assert_eq!(response.daily_series.len(), 30);
    let may_1 = response
        .daily_series
        .iter()
        .find(|point| point.date == "2026-05-01")
        .unwrap();
    assert!((may_1.total_cost_usd - 0.30).abs() < 1e-9);
    assert_eq!(may_1.invocations, 2);
    assert_eq!(may_1.sources.len(), 2);
    assert_eq!(may_1.models[0].model, "opus");
    assert_eq!(may_1.models[1].model, "sonnet");
    assert!(
        response
            .daily_series
            .iter()
            .all(|point| point.date != "2026-04-01")
    );
}

#[test]
fn usage_overview_warns_and_skips_malformed_model_json_in_daily_series() {
    let dir = tempdir().unwrap();
    let conn = open_connection(dir.path(), true).unwrap();
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, input_tokens, output_tokens,
            cache_creation_tokens, cache_read_tokens, web_search_requests,
            web_fetch_requests, model_usage_json, api_key_source
         ) VALUES (
            '2026-05-23T08:00:00Z', 'interactive', 1, 0, NULL, 'bad-json',
            0.50, 1, 10, 20, 5, 40, 1, 2, '{not-json', 'none'
         )",
        [],
    )
    .unwrap();

    let response = usage_overview(
        &conn,
        UsageOverviewInput {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-23T12:00:00Z".to_owned(),
        },
    )
    .unwrap();

    let today = response
        .daily_series
        .iter()
        .find(|point| point.date == "2026-05-23")
        .unwrap();
    assert!((today.total_cost_usd - 0.50).abs() < 1e-9);
    assert!(today.models.is_empty());
    assert_eq!(response.warnings[0].source, "usage_events.model_usage_json");
    assert_eq!(response.warnings[0].kind, "malformed_json");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview_builds_daily_series_for_last_30_days usage_overview_warns_and_skips_malformed_model_json_in_daily_series
```

Expected: FAIL because `UsageOverviewResponse` fields are not populated yet.

- [ ] **Step 3: Implement daily series helpers**

In `crates/right-dashboard/src/read_model/usage.rs`, import the new DTOs:

```rust
// crates/right-dashboard/src/read_model/usage.rs
use crate::api_types::{
    CostSeriesPoint, DashboardDataWarning, UsageDailyPoint, UsageModelSummary, UsageOverviewResponse,
    UsageSourcePoint, UsageSourceSeries, UsageSourceSummary, UsageWindow,
};
use chrono::NaiveDate;
```

Add constants:

```rust
// crates/right-dashboard/src/read_model/usage.rs
const DEFAULT_CHART_WINDOW: &str = "last_30_days";
const DAILY_SERIES_DAYS: i64 = 30;
```

Update `usage_overview` to build chart fields:

```rust
// crates/right-dashboard/src/read_model/usage.rs
let (daily_series, warnings) = build_daily_series(conn, &now)?;
let source_series = build_source_series(&daily_series);

Ok(UsageOverviewResponse {
    agent: input.agent,
    generated_at: input.generated_at,
    windows,
    selected_window: DEFAULT_CHART_WINDOW.to_owned(),
    daily_series,
    source_series,
    warnings,
})
```

Add these helpers below `build_window`:

```rust
// crates/right-dashboard/src/read_model/usage.rs
fn build_daily_series(
    conn: &Connection,
    now: &DateTime<Utc>,
) -> Result<(Vec<UsageDailyPoint>, Vec<DashboardDataWarning>), ReadModelError> {
    let mut warnings = Vec::new();
    let start_date = (now.date_naive() - Duration::days(DAILY_SERIES_DAYS - 1))
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(now.to_rfc3339()))?;
    let since = Utc.from_utc_datetime(&start_date).to_rfc3339();

    let mut points = (0..DAILY_SERIES_DAYS)
        .map(|offset| {
            let date = (now.date_naive() - Duration::days(DAILY_SERIES_DAYS - 1 - offset))
                .format("%Y-%m-%d")
                .to_string();
            UsageDailyPoint {
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
            }
        })
        .collect::<Vec<_>>();

    let mut by_date = std::collections::BTreeMap::<String, usize>::new();
    for (idx, point) in points.iter().enumerate() {
        by_date.insert(point.date.clone(), idx);
    }

    let mut stmt = conn.prepare(
        "SELECT ts, source, total_cost_usd, num_turns, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, web_search_requests,
                web_fetch_requests, model_usage_json, api_key_source
         FROM usage_events
         WHERE ts >= ?1
         ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
        ))
    })?;

    let mut source_totals = std::collections::BTreeMap::<(String, String), UsageSourcePoint>::new();
    let mut model_totals =
        std::collections::BTreeMap::<String, std::collections::BTreeMap<String, UsageModelSummary>>::new();

    for row in rows {
        let (
            ts,
            source,
            cost,
            turns,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            web_search_requests,
            web_fetch_requests,
            model_usage_json,
            api_key_source,
        ) = row?;
        let date = ts.get(0..10).unwrap_or(ts.as_str()).to_owned();
        let Some(idx) = by_date.get(&date).copied() else {
            continue;
        };

        let point = &mut points[idx];
        point.total_cost_usd += cost;
        if api_key_source == "none" {
            point.subscription_cost_usd += cost;
        } else {
            point.api_cost_usd += cost;
        }
        point.turns += turns.max(0) as u64;
        point.invocations += 1;
        point.input_tokens += input_tokens.max(0) as u64;
        point.output_tokens += output_tokens.max(0) as u64;
        point.cache_creation_tokens += cache_creation_tokens.max(0) as u64;
        point.cache_read_tokens += cache_read_tokens.max(0) as u64;
        point.web_search_requests += web_search_requests.max(0) as u64;
        point.web_fetch_requests += web_fetch_requests.max(0) as u64;

        let source_entry = source_totals
            .entry((date.clone(), source.clone()))
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
            });
        source_entry.cost_usd += cost;
        if api_key_source == "none" {
            source_entry.subscription_cost_usd += cost;
        } else {
            source_entry.api_cost_usd += cost;
        }
        source_entry.turns += turns.max(0) as u64;
        source_entry.invocations += 1;

        match parse_model_usage_for_daily(&model_usage_json) {
            Ok(models) => {
                let date_models = model_totals.entry(date).or_default();
                for model in models {
                    let entry = date_models
                        .entry(model.model.clone())
                        .or_insert_with(|| UsageModelSummary {
                            model: model.model.clone(),
                            cost_usd: 0.0,
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_creation_tokens: 0,
                            cache_read_tokens: 0,
                        });
                    entry.cost_usd += model.cost_usd;
                    entry.input_tokens += model.input_tokens;
                    entry.output_tokens += model.output_tokens;
                    entry.cache_creation_tokens += model.cache_creation_tokens;
                    entry.cache_read_tokens += model.cache_read_tokens;
                }
            }
            Err(_) => warnings.push(DashboardDataWarning {
                source: "usage_events.model_usage_json".to_owned(),
                kind: "malformed_json".to_owned(),
                message: format!("skipped malformed model usage JSON for usage event at {ts}"),
            }),
        }
    }

    for point in &mut points {
        point.sources = source_totals
            .iter()
            .filter_map(|((date, _source), value)| (date == &point.date).then_some(value.clone()))
            .collect();
        point.sources.sort_by(|left, right| left.source.cmp(&right.source));
        point.models = model_totals
            .remove(&point.date)
            .map(|models| {
                let mut rows = models.into_values().collect::<Vec<_>>();
                sort_models(&mut rows);
                rows
            })
            .unwrap_or_default();
    }

    Ok((points, warnings))
}

fn build_source_series(points: &[UsageDailyPoint]) -> Vec<UsageSourceSeries> {
    let mut series = std::collections::BTreeMap::<String, Vec<CostSeriesPoint>>::new();
    for point in points {
        for source in &point.sources {
            series
                .entry(source.source.clone())
                .or_default()
                .push(CostSeriesPoint {
                    bucket: point.date.clone(),
                    cost_usd: source.cost_usd,
                });
        }
    }
    series
        .into_iter()
        .map(|(source, points)| UsageSourceSeries { source, points })
        .collect()
}

fn parse_model_usage_for_daily(raw: &str) -> Result<Vec<UsageModelSummary>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let Some(models) = value.as_object() else {
        return Ok(Vec::new());
    };
    Ok(models
        .iter()
        .map(|(model, fields)| UsageModelSummary {
            model: model.clone(),
            cost_usd: field_f64(fields, "costUSD"),
            input_tokens: field_u64(fields, "inputTokens"),
            output_tokens: field_u64(fields, "outputTokens"),
            cache_creation_tokens: field_u64(fields, "cacheCreationInputTokens"),
            cache_read_tokens: field_u64(fields, "cacheReadInputTokens"),
        })
        .collect())
}
```

- [ ] **Step 4: Run targeted usage tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview
```

Expected: PASS.

- [ ] **Step 5: Commit usage read model**

```bash
git add crates/right-dashboard/src/read_model/usage.rs
git commit -m "feat(dashboard): project usage spend series"
```

## Task 3: Overview Signals And Cost/Learning River

**Files:**
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs`

- [ ] **Step 1: Write failing overview visual tests**

Append this test in `crates/right-dashboard/src/read_model/dashboard_overview.rs`:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs

#[test]
fn dashboard_overview_builds_signal_timeline_and_cost_river() {
    let (_dir, conn) = fixture();
    conn.execute(
        "INSERT INTO async_runs (
            id, kind, producer_ref, run_session_id, target_chat_id,
            status, finished_at, exit_code, delivery_required, delivery_status,
            created_at, updated_at
         ) VALUES (
            'run-failed', 'cron', 'daily', 'session-failed', 123,
            'failed', '2026-05-23T07:00:00Z', 1, 1, 'pending',
            '2026-05-23T07:00:00Z', '2026-05-23T07:00:00Z'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO usage_events (
            ts, source, chat_id, thread_id, job_name, session_uuid,
            total_cost_usd, num_turns, model_usage_json, api_key_source
         ) VALUES
            ('2026-05-21T08:00:00Z', 'interactive', 1, 0, NULL, 's1', 0.10, 1, '{}', 'none'),
            ('2026-05-22T08:00:00Z', 'interactive', 1, 0, NULL, 's2', 0.10, 1, '{}', 'none'),
            ('2026-05-23T08:00:00Z', 'learning_probe_writer', 1, 0, NULL, 's3', 1.00, 1, '{}', 'none')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO skill_learning_events (
            invocation_id, agent_name, action, skill_name, phase, status,
            message, summary, event_refs_json, created_at
         ) VALUES (
            'inv-2', 'alpha', 'create', 'rightx-debug', 'finish',
            'created', 'Learned debugging.', 'Reusable debugging workflow.',
            '[]', '2026-05-23T09:00:00Z'
         )",
        [],
    )
    .unwrap();

    let response = dashboard_overview(
        &conn,
        DashboardOverviewInput {
            agent: "alpha".to_string(),
            generated_at: "2026-05-23T10:00:00Z".to_string(),
            foreground_active_count: 1,
            sandbox: crate::api_types::OverviewSandboxStatus {
                state: "configured".to_string(),
                detail: Some("sandbox alpha".to_string()),
            },
        },
    )
    .unwrap();

    assert!(response.signals.len() >= 3);
    assert_eq!(response.signals[0].kind, "learning_outcome");
    assert!(
        response
            .signals
            .iter()
            .any(|signal| signal.kind == "cost_spike")
    );
    assert!(
        response
            .signals
            .iter()
            .any(|signal| signal.kind == "run_failure")
    );
    assert_eq!(response.cost_learning_river.window, "last_30_days");
    assert!(
        response
            .cost_learning_river
            .series
            .iter()
            .any(|series| series.source == "learning_probe_writer")
    );
    assert_eq!(response.cost_learning_river.markers[0].kind, "skill_created");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_overview_builds_signal_timeline_and_cost_river
```

Expected: FAIL because overview visual fields are empty.

- [ ] **Step 3: Implement overview projections**

In `crates/right-dashboard/src/read_model/dashboard_overview.rs`, expand imports:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs
use crate::api_types::{
    CostLearningPoint, CostLearningRiver, CostLearningSeries, CostSeriesPoint, DashboardDataWarning,
    DashboardOverviewResponse, DashboardSignal, LearningMarker, OverviewDoctorStatus,
    OverviewSandboxStatus, UsageSourcePoint,
};
use chrono::{Duration, NaiveDate};
use std::collections::BTreeMap;
```

Add constants:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs
const SIGNAL_LIMIT: usize = 30;
const RIVER_DAYS: i64 = 30;
const RIVER_WINDOW: &str = "last_30_days";
```

Before returning `DashboardOverviewResponse`, compute:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs
let (cost_learning_river, mut warnings) =
    cost_learning_river(conn, &input.generated_at, &input.agent)?;
let mut signals = overview_signals(
    conn,
    &input.agent,
    &input.generated_at,
    input.foreground_active_count,
    &input.sandbox,
    &cost_learning_river,
)?;
signals.truncate(SIGNAL_LIMIT);
```

Add the new response fields:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs
signals,
cost_learning_river,
warnings,
```

Add helper functions below `learning_candidate_count`:

```rust
// crates/right-dashboard/src/read_model/dashboard_overview.rs
fn overview_signals(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
    foreground_active_count: i64,
    sandbox: &OverviewSandboxStatus,
    river: &CostLearningRiver,
) -> Result<Vec<DashboardSignal>, ReadModelError> {
    let since_24h = window_start(generated_at, Duration::hours(24))?;
    let mut signals = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT id, kind, producer_ref, status, COALESCE(finished_at, updated_at, created_at)
         FROM async_runs
         WHERE status = 'failed' AND COALESCE(finished_at, updated_at, created_at) >= ?1
         ORDER BY COALESCE(finished_at, updated_at, created_at) DESC
         LIMIT 10",
    )?;
    let failed_runs = stmt.query_map(params![since_24h], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in failed_runs {
        let (id, kind, producer_ref, status, occurred_at) = row?;
        signals.push(DashboardSignal {
            id: format!("run_failure:{id}"),
            kind: "run_failure".to_owned(),
            severity: "bad".to_owned(),
            occurred_at,
            title: format!("{kind} run failed"),
            detail: producer_ref.or(Some(status)),
            source: Some(kind),
            cost_usd: None,
            related_run_id: Some(id),
            related_skill_name: None,
            related_report_id: None,
        });
    }

    let mut stmt = conn.prepare(
        "SELECT skill_name, action, status, message, created_at
         FROM skill_learning_events
         WHERE agent_name = ?1
           AND phase = 'finish'
           AND created_at >= ?2
         ORDER BY created_at DESC
         LIMIT 10",
    )?;
    let learning_events = stmt.query_map(params![agent, since_24h], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in learning_events {
        let (skill_name, action, status, message, occurred_at) = row?;
        signals.push(DashboardSignal {
            id: format!("learning:{skill_name}:{occurred_at}"),
            kind: "learning_outcome".to_owned(),
            severity: if status == "failed" || status == "aborted" {
                "bad".to_owned()
            } else if status == "refused" {
                "warn".to_owned()
            } else {
                "info".to_owned()
            },
            occurred_at,
            title: format!("Skill {status}"),
            detail: message,
            source: Some(action),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: Some(skill_name),
            related_report_id: None,
        });
    }

    for spike in cost_spike_signals(river) {
        signals.push(spike);
    }

    if foreground_active_count > 0 {
        signals.push(DashboardSignal {
            id: format!("active_work:foreground:{generated_at}"),
            kind: "active_work".to_owned(),
            severity: "info".to_owned(),
            occurred_at: generated_at.to_owned(),
            title: "Foreground work active".to_owned(),
            detail: Some(format!("{foreground_active_count} active foreground sessions")),
            source: Some("foreground".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        });
    }

    if sandbox.state == "warn" || sandbox.state == "unavailable" {
        signals.push(DashboardSignal {
            id: format!("health:sandbox:{generated_at}"),
            kind: "health".to_owned(),
            severity: if sandbox.state == "unavailable" { "bad" } else { "warn" }.to_owned(),
            occurred_at: generated_at.to_owned(),
            title: "Sandbox warning".to_owned(),
            detail: sandbox.detail.clone(),
            source: Some("sandbox".to_owned()),
            cost_usd: None,
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        });
    }

    signals.sort_by(|left, right| right.occurred_at.cmp(&left.occurred_at));
    Ok(signals)
}

fn cost_learning_river(
    conn: &Connection,
    generated_at: &str,
    agent: &str,
) -> Result<(CostLearningRiver, Vec<DashboardDataWarning>), ReadModelError> {
    let now = chrono::DateTime::parse_from_rfc3339(generated_at)?.with_timezone(&chrono::Utc);
    let start_date = now.date_naive() - Duration::days(RIVER_DAYS - 1);
    let since = start_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ReadModelError::InvalidStartOfDay(generated_at.to_owned()))?;
    let since = chrono::Utc.from_utc_datetime(&since).to_rfc3339();

    let mut points = (0..RIVER_DAYS)
        .map(|offset| CostLearningPoint {
            bucket: (start_date + Duration::days(offset))
                .format("%Y-%m-%d")
                .to_string(),
            total_cost_usd: 0.0,
            sources: Vec::new(),
        })
        .collect::<Vec<_>>();
    let mut index = BTreeMap::new();
    for (idx, point) in points.iter().enumerate() {
        index.insert(point.bucket.clone(), idx);
    }

    let mut source_totals = BTreeMap::<(String, String), UsageSourcePoint>::new();
    let mut stmt = conn.prepare(
        "SELECT ts, source, total_cost_usd, num_turns, api_key_source
         FROM usage_events
         WHERE ts >= ?1
         ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (ts, source, cost, turns, api_key_source) = row?;
        let bucket = ts.get(0..10).unwrap_or(ts.as_str()).to_owned();
        let Some(idx) = index.get(&bucket).copied() else {
            continue;
        };
        points[idx].total_cost_usd += cost;
        let entry = source_totals
            .entry((bucket, source.clone()))
            .or_insert_with(|| UsageSourcePoint {
                source: source.clone(),
                cost_usd: 0.0,
                subscription_cost_usd: 0.0,
                api_cost_usd: 0.0,
                turns: 0,
                invocations: 0,
            });
        entry.cost_usd += cost;
        if api_key_source == "none" {
            entry.subscription_cost_usd += cost;
        } else {
            entry.api_cost_usd += cost;
        }
        entry.turns += turns.max(0) as u64;
        entry.invocations += 1;
    }

    for point in &mut points {
        point.sources = source_totals
            .iter()
            .filter_map(|((bucket, _), value)| (bucket == &point.bucket).then_some(value.clone()))
            .collect();
    }

    let mut by_source = BTreeMap::<String, Vec<CostSeriesPoint>>::new();
    for point in &points {
        for source in &point.sources {
            by_source
                .entry(source.source.clone())
                .or_default()
                .push(CostSeriesPoint {
                    bucket: point.bucket.clone(),
                    cost_usd: source.cost_usd,
                });
        }
    }

    let markers = learning_markers(conn, agent, &since)?;
    Ok((
        CostLearningRiver {
            window: RIVER_WINDOW.to_owned(),
            points,
            series: by_source
                .into_iter()
                .map(|(source, points)| CostLearningSeries { source, points })
                .collect(),
            markers,
        },
        Vec::new(),
    ))
}

fn learning_markers(
    conn: &Connection,
    agent: &str,
    since: &str,
) -> Result<Vec<LearningMarker>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT skill_name, status, action, created_at
         FROM skill_learning_events
         WHERE agent_name = ?1 AND phase = 'finish' AND created_at >= ?2
         ORDER BY created_at DESC
         LIMIT 30",
    )?;
    let rows = stmt.query_map(params![agent, since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    let mut markers = Vec::new();
    for row in rows {
        let (skill_name, status, action, occurred_at) = row?;
        markers.push(LearningMarker {
            id: format!("marker:{skill_name}:{occurred_at}"),
            occurred_at,
            kind: format!("skill_{status}"),
            label: skill_name.clone(),
            severity: if status == "failed" || status == "aborted" {
                "bad".to_owned()
            } else if status == "refused" {
                "warn".to_owned()
            } else {
                "info".to_owned()
            },
            skill_name: Some(skill_name),
            source: Some(action),
            cost_usd: None,
        });
    }
    Ok(markers)
}

fn cost_spike_signals(river: &CostLearningRiver) -> Vec<DashboardSignal> {
    let mut non_zero = river
        .points
        .iter()
        .filter_map(|point| (point.total_cost_usd > 0.0).then_some(point.total_cost_usd))
        .collect::<Vec<_>>();
    non_zero.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    if non_zero.is_empty() {
        return Vec::new();
    }
    let median = non_zero[non_zero.len() / 2];
    river
        .points
        .iter()
        .filter(|point| median > 0.0 && point.total_cost_usd >= median * 2.0)
        .map(|point| DashboardSignal {
            id: format!("cost_spike:{}", point.bucket),
            kind: "cost_spike".to_owned(),
            severity: "warn".to_owned(),
            occurred_at: format!("{}T00:00:00Z", point.bucket),
            title: "Cost spike".to_owned(),
            detail: Some(format!("Daily cost ${:.2}", point.total_cost_usd)),
            source: None,
            cost_usd: Some(point.total_cost_usd),
            related_run_id: None,
            related_skill_name: None,
            related_report_id: None,
        })
        .collect()
}
```

- [ ] **Step 4: Run overview tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_overview
```

Expected: PASS.

- [ ] **Step 5: Commit overview read model**

```bash
git add crates/right-dashboard/src/read_model/dashboard_overview.rs
git commit -m "feat(dashboard): project overview signals"
```

## Task 4: Learning Flow Read Model

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning.rs`

- [ ] **Step 1: Write failing learning-flow tests**

Append this test in `crates/right-dashboard/src/read_model/learning.rs` tests module:

```rust
// crates/right-dashboard/src/read_model/learning.rs

#[test]
fn learning_overview_builds_flow_nodes_edges_and_recent_signals() {
    let (_dir, conn) = fixture();
    conn.execute(
        "INSERT INTO skill_nudge_signals (
            invocation_id, agent_name, root_session_id, chat_id, thread_id,
            signal_kind, payload_json, accepted_at
         ) VALUES (
            'inv-1', 'right', 'session-1', 10, 20,
            'learning', '{}', '2026-05-20T10:00:00Z'
         )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO skill_learning_events (
            invocation_id, agent_name, action, skill_name, phase, status,
            message, summary, event_refs_json, hint_outcome, created_at
         ) VALUES (
            'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
            'created', 'Learned OAuth callback verification.',
            'Reusable OAuth setup workflow.', '[]', 'applied_as_hinted',
            '2026-05-20T11:10:00Z'
         )",
        [],
    )
    .unwrap();

    let response = learning_overview(&conn, input()).unwrap();

    assert!(
        response
            .flow_nodes
            .iter()
            .any(|node| node.id == "signals" && node.count == 1)
    );
    assert!(
        response
            .flow_nodes
            .iter()
            .any(|node| node.id == "writer_created" && node.count == 1)
    );
    assert!(
        response
            .flow_edges
            .iter()
            .any(|edge| edge.source == "signals" && edge.target == "writer_created")
    );
    assert_eq!(response.recent_learning_signals[0].kind, "skill_created");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_overview_builds_flow_nodes_edges_and_recent_signals
```

Expected: FAIL because flow fields are empty.

- [ ] **Step 3: Implement learning flow helpers**

In `crates/right-dashboard/src/read_model/learning.rs`, import the new DTOs:

```rust
// crates/right-dashboard/src/read_model/learning.rs
use crate::api_types::{
    DashboardDataWarning, LearningCapabilities, LearningEpisodeDetail, LearningEventSummary,
    LearningEvidenceSnippet, LearningFlowEdge, LearningFlowNode, LearningFunnel, LearningHealth,
    LearningLifecycle, LearningOverviewResponse, LearningQuality, LearningReportDetailResponse,
    LearningReportSummary, LearningReviewerDetail, LearningSelectorDetail, LearningSignalPoint,
};
```

Near the end of `learning_overview`, before the `Ok`, compute:

```rust
// crates/right-dashboard/src/read_model/learning.rs
let flow_nodes = learning_flow_nodes(
    signals_accepted_24h,
    create_candidates_24h,
    update_candidates_24h,
    nothing_to_learn_24h,
    failed_reviews_24h,
    &lifecycle,
);
let flow_edges = learning_flow_edges(&flow_nodes);
let recent_learning_signals = recent_learning_signals(conn, agent, &since_7d)?;
let warnings = Vec::<DashboardDataWarning>::new();
```

Add the new fields to `LearningOverviewResponse`.

Add these helpers before `skill_lifecycle_overview`:

```rust
// crates/right-dashboard/src/read_model/learning.rs
fn learning_flow_nodes(
    signals: i64,
    create_candidates: i64,
    update_candidates: i64,
    nothing_to_learn: i64,
    failed_reviews: i64,
    lifecycle: &LearningLifecycle,
) -> Vec<LearningFlowNode> {
    vec![
        LearningFlowNode {
            id: "signals".to_owned(),
            label: "Signals".to_owned(),
            kind: "signal".to_owned(),
            count: signals,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_create".to_owned(),
            label: "Create candidates".to_owned(),
            kind: "prefilter".to_owned(),
            count: create_candidates,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_patch".to_owned(),
            label: "Patch candidates".to_owned(),
            kind: "prefilter".to_owned(),
            count: update_candidates,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "prefilter_skip".to_owned(),
            label: "Nothing to learn".to_owned(),
            kind: "prefilter".to_owned(),
            count: nothing_to_learn,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_created".to_owned(),
            label: "Created".to_owned(),
            kind: "writer".to_owned(),
            count: lifecycle.created_7d,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_updated".to_owned(),
            label: "Updated".to_owned(),
            kind: "writer".to_owned(),
            count: lifecycle.updated_7d,
            severity: "info".to_owned(),
        },
        LearningFlowNode {
            id: "writer_failed".to_owned(),
            label: "Failed or aborted".to_owned(),
            kind: "writer".to_owned(),
            count: lifecycle.failed_or_aborted_7d + failed_reviews,
            severity: "bad".to_owned(),
        },
    ]
}

fn learning_flow_edges(nodes: &[LearningFlowNode]) -> Vec<LearningFlowEdge> {
    let count = |id: &str| -> i64 {
        nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.count)
            .unwrap_or(0)
    };
    let mut edges = Vec::new();
    for target in ["prefilter_create", "prefilter_patch", "prefilter_skip"] {
        let value = count(target);
        if value > 0 {
            edges.push(LearningFlowEdge {
                source: "signals".to_owned(),
                target: target.to_owned(),
                count: value,
            });
        }
    }
    if count("writer_created") > 0 {
        edges.push(LearningFlowEdge {
            source: "prefilter_create".to_owned(),
            target: "writer_created".to_owned(),
            count: count("writer_created"),
        });
    }
    if count("writer_updated") > 0 {
        edges.push(LearningFlowEdge {
            source: "prefilter_patch".to_owned(),
            target: "writer_updated".to_owned(),
            count: count("writer_updated"),
        });
    }
    if count("writer_failed") > 0 {
        edges.push(LearningFlowEdge {
            source: "signals".to_owned(),
            target: "writer_failed".to_owned(),
            count: count("writer_failed"),
        });
    }
    edges
}

fn recent_learning_signals(
    conn: &Connection,
    agent: &str,
    since: &str,
) -> Result<Vec<LearningSignalPoint>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT skill_name, status, created_at
         FROM skill_learning_events
         WHERE agent_name = ?1 AND phase = 'finish' AND created_at >= ?2
         ORDER BY created_at DESC
         LIMIT 30",
    )?;
    let rows = stmt.query_map(params![agent, since], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut signals = Vec::new();
    for row in rows {
        let (skill_name, status, occurred_at) = row?;
        signals.push(LearningSignalPoint {
            id: format!("learning:{skill_name}:{occurred_at}"),
            occurred_at,
            kind: format!("skill_{status}"),
            label: skill_name.clone(),
            severity: if status == "failed" || status == "aborted" {
                "bad".to_owned()
            } else if status == "refused" {
                "warn".to_owned()
            } else {
                "info".to_owned()
            },
            skill_name: Some(skill_name),
            count: 1,
        });
    }
    Ok(signals)
}
```

- [ ] **Step 4: Run learning tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_overview
```

Expected: PASS.

- [ ] **Step 5: Commit learning flow read model**

```bash
git add crates/right-dashboard/src/read_model/learning.rs
git commit -m "feat(dashboard): project learning flow"
```

## Task 5: API Handler Regression Tests

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Find current dashboard handler tests**

Run:

```bash
devenv shell -- rg -n "overview_returns|usage|learning_overview_returns|rejects_missing_auth" crates/bot/src/telegram/dashboard.rs
```

Expected: existing authorized and auth-rejection tests are in `crates/bot/src/telegram/dashboard.rs`.

- [ ] **Step 2: Add assertions to existing authorized tests**

In the existing authorized overview test, add assertions like:

```rust
// crates/bot/src/telegram/dashboard.rs
assert!(body.get("signals").is_some(), "overview must expose visual signals");
assert!(
    body.get("cost_learning_river").is_some(),
    "overview must expose cost_learning_river"
);
assert!(body.get("warnings").is_some(), "overview must expose warnings");
```

In the existing authorized usage test, add:

```rust
// crates/bot/src/telegram/dashboard.rs
assert_eq!(body["selected_window"], "last_30_days");
assert!(body.get("daily_series").is_some(), "usage must expose daily_series");
assert!(body.get("source_series").is_some(), "usage must expose source_series");
```

In the existing learning overview test, add:

```rust
// crates/bot/src/telegram/dashboard.rs
assert!(body.get("flow_nodes").is_some(), "learning overview must expose flow_nodes");
assert!(body.get("flow_edges").is_some(), "learning overview must expose flow_edges");
assert!(
    body.get("recent_learning_signals").is_some(),
    "learning overview must expose recent_learning_signals"
);
```

- [ ] **Step 3: Run dashboard handler tests**

Run:

```bash
devenv shell -- cargo test -p right-bot dashboard::tests::overview dashboard::tests::usage dashboard::tests::learning_overview dashboard::tests::learning_overview_rejects_missing_auth
```

Expected: PASS. If the module path differs, rerun the exact test names printed by `cargo test -p right-bot -- --list | rg "overview|usage|learning_overview"`.

- [ ] **Step 4: Commit handler tests**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "test(dashboard): cover visual analytics responses"
```

## Task 6: Frontend Dependencies And Types

**Files:**
- Modify: `crates/right-dashboard/frontend/package.json`
- Modify: `crates/right-dashboard/frontend/package-lock.json`
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Create: `crates/right-dashboard/frontend/src/charts.ts`

- [ ] **Step 1: Add chart dependencies**

Run:

```bash
cd crates/right-dashboard/frontend
npm install echarts vue-echarts
```

Expected: `package.json` and `package-lock.json` include `echarts` and `vue-echarts`.

- [ ] **Step 2: Add TypeScript DTOs**

Add these interfaces to `crates/right-dashboard/frontend/src/types.ts`, matching the Rust DTOs exactly:

```ts
// crates/right-dashboard/frontend/src/types.ts

export interface DashboardDataWarning {
  source: string
  kind: string
  message: string
}

export interface DashboardSignal {
  id: string
  kind: string
  severity: string
  occurred_at: string
  title: string
  detail: string | null
  source: string | null
  cost_usd: number | null
  related_run_id: string | null
  related_skill_name: string | null
  related_report_id: number | null
}

export interface CostLearningRiver {
  window: string
  points: CostLearningPoint[]
  series: CostLearningSeries[]
  markers: LearningMarker[]
}

export interface CostLearningPoint {
  bucket: string
  total_cost_usd: number
  sources: UsageSourcePoint[]
}

export interface CostLearningSeries {
  source: string
  points: CostSeriesPoint[]
}

export interface CostSeriesPoint {
  bucket: string
  cost_usd: number
}

export interface LearningMarker {
  id: string
  occurred_at: string
  kind: string
  label: string
  severity: string
  skill_name: string | null
  source: string | null
  cost_usd: number | null
}

export interface UsageDailyPoint {
  date: string
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
  sources: UsageSourcePoint[]
  models: UsageModelSummary[]
}

export interface UsageSourcePoint {
  source: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
}

export interface UsageSourceSeries {
  source: string
  points: CostSeriesPoint[]
}

export interface LearningFlowNode {
  id: string
  label: string
  kind: string
  count: number
  severity: string
}

export interface LearningFlowEdge {
  source: string
  target: string
  count: number
}

export interface LearningSignalPoint {
  id: string
  occurred_at: string
  kind: string
  label: string
  severity: string
  skill_name: string | null
  count: number
}
```

Extend existing interfaces:

```ts
// crates/right-dashboard/frontend/src/types.ts

export interface DashboardOverviewResponse {
  agent: string
  generated_at: string
  active_runs: number
  recent_failures: number
  today_cost_usd: number
  learning_candidates_24h: number
  doctor: OverviewDoctorStatus
  sandbox: OverviewSandboxStatus
  signals: DashboardSignal[]
  cost_learning_river: CostLearningRiver
  warnings: DashboardDataWarning[]
}

export interface UsageOverviewResponse {
  agent: string
  generated_at: string
  windows: UsageWindow[]
  selected_window: string
  daily_series: UsageDailyPoint[]
  source_series: UsageSourceSeries[]
  warnings: DashboardDataWarning[]
}

export interface LearningOverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  capabilities: LearningCapabilities
  funnel: LearningFunnel
  quality: LearningQuality
  health: LearningHealth
  lifecycle: LearningLifecycle
  recent_reports: LearningReportSummary[]
  flow_nodes: LearningFlowNode[]
  flow_edges: LearningFlowEdge[]
  recent_learning_signals: LearningSignalPoint[]
  warnings: DashboardDataWarning[]
}
```

- [ ] **Step 3: Register ECharts modules once**

Create `crates/right-dashboard/frontend/src/charts.ts`:

```ts
// crates/right-dashboard/frontend/src/charts.ts
import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { BarChart, LineChart, SankeyChart, ThemeRiverChart } from 'echarts/charts'
import {
  DatasetComponent,
  DataZoomComponent,
  GraphicComponent,
  GridComponent,
  LegendComponent,
  TooltipComponent,
} from 'echarts/components'

let registered = false

export function registerDashboardCharts(): void {
  if (registered) {
    return
  }
  use([
    CanvasRenderer,
    BarChart,
    LineChart,
    ThemeRiverChart,
    SankeyChart,
    DatasetComponent,
    DataZoomComponent,
    GraphicComponent,
    GridComponent,
    LegendComponent,
    TooltipComponent,
  ])
  registered = true
}
```

- [ ] **Step 4: Run frontend typecheck**

Run:

```bash
cd crates/right-dashboard/frontend
npm run typecheck
```

Expected: PASS.

- [ ] **Step 5: Commit dependencies and types**

```bash
git add crates/right-dashboard/frontend/package.json crates/right-dashboard/frontend/package-lock.json crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/charts.ts
git commit -m "feat(dashboard): add chart dependencies and types"
```

## Task 7: Overview Chart Components

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue`
- Create: `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue`
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue`
- Modify: `crates/right-dashboard/frontend/src/App.vue`

- [ ] **Step 1: Create signal timeline component**

Create `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue -->
<script setup lang="ts">
import StatusPill from '../StatusPill.vue'
import { money, shortDate } from '../../format'
import type { DashboardSignal } from '../../types'

defineProps<{
  signals: DashboardSignal[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [signal: DashboardSignal]
}>()
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Signals</p>
        <h2>Recent changes</h2>
      </div>
    </header>

    <div v-if="signals.length === 0" class="chart-empty">No recent signals</div>
    <button
      v-for="signal in signals"
      v-else
      :key="signal.id"
      type="button"
      class="data-row tall"
      :class="{ selected: selectedId === signal.id }"
      @click="emit('select', signal)"
    >
      <span class="row-main">
        <strong>{{ signal.title }}</strong>
        <span>{{ signal.detail ?? signal.source ?? signal.kind }}</span>
        <small>{{ shortDate(signal.occurred_at) }}</small>
      </span>
      <span class="row-side">
        <StatusPill :status="signal.severity" />
        <small v-if="signal.cost_usd !== null">{{ money(signal.cost_usd) }}</small>
      </span>
    </button>
  </section>
</template>
```

- [ ] **Step 2: Create cost/learning river component**

Create `crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'
import { registerDashboardCharts } from '../../charts'
import { money } from '../../format'
import type { CostLearningRiver, LearningMarker } from '../../types'

registerDashboardCharts()

const props = defineProps<{
  river: CostLearningRiver | null
}>()

const emit = defineEmits<{
  selectMarker: [marker: LearningMarker]
}>()

const option = computed(() => {
  const river = props.river
  if (!river || river.points.length === 0) {
    return null
  }
  const data = river.points.flatMap((point) =>
    point.sources.map((source) => [point.bucket, source.cost_usd, source.source]),
  )
  return {
    tooltip: {
      trigger: 'axis',
      formatter: (params: unknown) => {
        const rows = Array.isArray(params) ? params : [params]
        return rows.map((row: any) => `${row.data?.[2] ?? row.seriesName}: ${money(row.data?.[1] ?? 0)}`).join('<br>')
      },
    },
    legend: { type: 'scroll', bottom: 0 },
    singleAxis: {
      type: 'time',
      top: 16,
      bottom: 40,
      axisLabel: { hideOverlap: true },
    },
    series: [
      {
        type: 'themeRiver',
        emphasis: { focus: 'series' },
        data,
      },
    ],
  }
})
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Cost and learning</p>
        <h2>{{ river?.window ?? 'last_30_days' }}</h2>
      </div>
    </header>
    <div v-if="!option" class="chart-empty">No cost data</div>
    <VChart v-else class="dashboard-chart" :option="option" autoresize />
    <div v-if="river?.markers.length" class="marker-list">
      <button
        v-for="marker in river.markers"
        :key="marker.id"
        type="button"
        class="marker-chip"
        @click="emit('selectMarker', marker)"
      >
        {{ marker.label }}
      </button>
    </div>
  </section>
</template>
```

- [ ] **Step 3: Wire OverviewView**

Modify `crates/right-dashboard/frontend/src/views/OverviewView.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/views/OverviewView.vue -->
<script setup lang="ts">
import { ref } from 'vue'
import CostLearningRiver from '../components/charts/CostLearningRiver.vue'
import SignalTimeline from '../components/charts/SignalTimeline.vue'
import MetricCard from '../components/MetricCard.vue'
import StatusPill from '../components/StatusPill.vue'
import { money, shortDate } from '../format'
import type { DashboardOverviewResponse, DashboardSignal, LearningMarker, OverviewResponse } from '../types'

defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
}>()

const selectedSignal = ref<DashboardSignal | null>(null)
const selectedMarker = ref<LearningMarker | null>(null)
</script>

<template>
  <section class="metric-grid">
    <MetricCard label="Active" :value="overview?.active_runs ?? 0" tone="active" />
    <MetricCard label="Failures" :value="overview?.recent_failures ?? 0" :tone="(overview?.recent_failures ?? 0) > 0 ? 'bad' : 'ok'" />
    <MetricCard label="Today" :value="money(overview?.today_cost_usd)" />
    <MetricCard label="Candidates" :value="overview?.learning_candidates_24h ?? 0" />
    <MetricCard label="Jobs" :value="activity?.summary.cron_count ?? 0" />
    <MetricCard label="Running cron" :value="activity?.summary.active_cron_count ?? 0" tone="active" />
  </section>

  <section v-if="overview?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span>{{ overview.warnings[0].message }}</span>
  </section>

  <section class="two-column wide-main">
    <SignalTimeline
      :signals="overview?.signals ?? []"
      :selected-id="selectedSignal?.id ?? null"
      @select="selectedSignal = $event"
    />
    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Selected signal</p>
          <h2>{{ selectedSignal?.title ?? selectedMarker?.label ?? 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedSignal" :status="selectedSignal.severity" />
      </header>
      <p v-if="!selectedSignal && !selectedMarker" class="muted-line">Select a signal or marker</p>
      <dl v-else class="meta-grid compact">
        <div>
          <dt>When</dt>
          <dd>{{ shortDate(selectedSignal?.occurred_at ?? selectedMarker?.occurred_at) }}</dd>
        </div>
        <div>
          <dt>Source</dt>
          <dd>{{ selectedSignal?.source ?? selectedMarker?.source ?? 'none' }}</dd>
        </div>
        <div>
          <dt>Skill</dt>
          <dd>{{ selectedSignal?.related_skill_name ?? selectedMarker?.skill_name ?? 'none' }}</dd>
        </div>
        <div>
          <dt>Cost</dt>
          <dd>{{ money(selectedSignal?.cost_usd ?? selectedMarker?.cost_usd) }}</dd>
        </div>
      </dl>
    </aside>
  </section>

  <CostLearningRiver
    :river="overview?.cost_learning_river ?? null"
    @select-marker="selectedMarker = $event"
  />
</template>
```

- [ ] **Step 4: Add shared chart CSS**

Append to the style block in `crates/right-dashboard/frontend/src/App.vue`:

```css
/* crates/right-dashboard/frontend/src/App.vue */
.chart-panel {
  min-height: 220px;
}

.dashboard-chart {
  width: 100%;
  height: 240px;
}

.chart-empty {
  display: grid;
  min-height: 180px;
  place-items: center;
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.84rem;
}

.marker-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
}

.marker-chip {
  min-height: 28px;
  padding: 4px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
}
```

- [ ] **Step 5: Run frontend typecheck**

Run:

```bash
cd crates/right-dashboard/frontend
npm run typecheck
```

Expected: PASS.

- [ ] **Step 6: Commit overview frontend**

```bash
git add crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue crates/right-dashboard/frontend/src/components/charts/CostLearningRiver.vue crates/right-dashboard/frontend/src/views/OverviewView.vue crates/right-dashboard/frontend/src/App.vue
git commit -m "feat(dashboard): render visual overview"
```

## Task 8: Usage Chart Components

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`
- Create: `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`

- [ ] **Step 1: Create UsageSpendChart**

Create `crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'
import { registerDashboardCharts } from '../../charts'
import { money } from '../../format'
import type { UsageDailyPoint } from '../../types'

registerDashboardCharts()

const props = defineProps<{
  points: UsageDailyPoint[]
  selectedDate: string | null
}>()

const emit = defineEmits<{
  selectDate: [date: string]
}>()

const sources = computed(() => Array.from(new Set(props.points.flatMap((point) => point.sources.map((source) => source.source)))))

const option = computed(() => ({
  tooltip: {
    trigger: 'axis',
    axisPointer: { type: 'shadow' },
    formatter: (params: unknown) => {
      const rows = Array.isArray(params) ? params : [params]
      return rows.map((row: any) => `${row.seriesName}: ${money(row.value ?? 0)}`).join('<br>')
    },
  },
  legend: { type: 'scroll', bottom: 0 },
  grid: { left: 44, right: 12, top: 18, bottom: 54 },
  xAxis: { type: 'category', data: props.points.map((point) => point.date), axisLabel: { hideOverlap: true } },
  yAxis: { type: 'value' },
  series: sources.value.map((source) => ({
    name: source,
    type: 'bar',
    stack: 'cost',
    emphasis: { focus: 'series' },
    data: props.points.map((point) => point.sources.find((row) => row.source === source)?.cost_usd ?? 0),
  })),
}))
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Spend</p>
        <h2>Last 30 days</h2>
      </div>
    </header>
    <div v-if="points.length === 0" class="chart-empty">No usage data</div>
    <VChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
      @click="(event: any) => emit('selectDate', String(event.name))"
    />
  </section>
</template>
```

- [ ] **Step 2: Create UsageBreakdown**

Create `crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue -->
<script setup lang="ts">
import { money } from '../../format'
import type { UsageDailyPoint } from '../../types'

defineProps<{
  point: UsageDailyPoint | null
}>()
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Breakdown</p>
        <h2>{{ point?.date ?? 'None selected' }}</h2>
      </div>
      <strong>{{ money(point?.total_cost_usd) }}</strong>
    </header>
    <p v-if="!point" class="muted-line">Select a day</p>
    <template v-else>
      <dl class="meta-grid compact">
        <div><dt>Subscription</dt><dd>{{ money(point.subscription_cost_usd) }}</dd></div>
        <div><dt>API</dt><dd>{{ money(point.api_cost_usd) }}</dd></div>
        <div><dt>Turns</dt><dd>{{ point.turns }}</dd></div>
        <div><dt>Calls</dt><dd>{{ point.invocations }}</dd></div>
      </dl>
      <section class="text-block">
        <h3>Sources</h3>
        <div class="row-list">
          <div v-for="source in point.sources" :key="source.source" class="model-row">
            <span>{{ source.source }}</span>
            <strong>{{ money(source.cost_usd) }}</strong>
          </div>
        </div>
      </section>
      <section class="text-block">
        <h3>Models</h3>
        <div class="row-list">
          <div v-for="model in point.models" :key="model.model" class="model-row">
            <span>{{ model.model }}</span>
            <strong>{{ money(model.cost_usd) }}</strong>
          </div>
        </div>
      </section>
    </template>
  </aside>
</template>
```

- [ ] **Step 3: Wire UsageView**

Replace `crates/right-dashboard/frontend/src/views/UsageView.vue` with:

```vue
<!-- crates/right-dashboard/frontend/src/views/UsageView.vue -->
<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import UsageBreakdown from '../components/charts/UsageBreakdown.vue'
import UsageSpendChart from '../components/charts/UsageSpendChart.vue'
import { money } from '../format'
import type { UsageOverviewResponse, UsageWindow } from '../types'

const props = defineProps<{
  usage: UsageOverviewResponse | null
}>()

const selectedDate = ref<string | null>(null)

watchEffect(() => {
  if (selectedDate.value === null && props.usage?.daily_series.length) {
    selectedDate.value = props.usage.daily_series[props.usage.daily_series.length - 1].date
  }
})

const selectedPoint = computed(() => props.usage?.daily_series.find((point) => point.date === selectedDate.value) ?? null)

function windowRows(window: UsageWindow | null | undefined) {
  return window?.sources ?? []
}
</script>

<template>
  <section v-if="usage?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span>{{ usage.warnings[0].message }}</span>
  </section>

  <section class="two-column wide-main">
    <UsageSpendChart
      :points="usage?.daily_series ?? []"
      :selected-date="selectedDate"
      @select-date="selectedDate = $event"
    />
    <UsageBreakdown :point="selectedPoint" />
  </section>

  <section class="list-stack">
    <article v-for="window in usage?.windows ?? []" :key="window.key" class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">{{ window.key }}</p>
          <h2>{{ window.label }}</h2>
        </div>
        <strong>{{ money(window.total_cost_usd) }}</strong>
      </header>
      <div class="model-grid">
        <div v-for="source in windowRows(window)" :key="source.source" class="model-row">
          <span>{{ source.source }}</span>
          <strong>{{ money(source.cost_usd) }}</strong>
        </div>
      </div>
    </article>
    <article v-if="!usage" class="empty-panel">No usage snapshot</article>
  </section>
</template>
```

- [ ] **Step 4: Run frontend typecheck**

Run:

```bash
cd crates/right-dashboard/frontend
npm run typecheck
```

Expected: PASS.

- [ ] **Step 5: Commit usage frontend**

```bash
git add crates/right-dashboard/frontend/src/components/charts/UsageSpendChart.vue crates/right-dashboard/frontend/src/components/charts/UsageBreakdown.vue crates/right-dashboard/frontend/src/views/UsageView.vue
git commit -m "feat(dashboard): render usage spend chart"
```

## Task 9: Knowledge Learning Flow Components

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/charts/LearningFlowChart.vue`
- Create: `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue`
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`

- [ ] **Step 1: Create LearningFlowChart**

Create `crates/right-dashboard/frontend/src/components/charts/LearningFlowChart.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/LearningFlowChart.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'
import { registerDashboardCharts } from '../../charts'
import type { LearningFlowEdge, LearningFlowNode } from '../../types'

registerDashboardCharts()

const props = defineProps<{
  nodes: LearningFlowNode[]
  edges: LearningFlowEdge[]
}>()

const emit = defineEmits<{
  selectNode: [nodeId: string]
}>()

const option = computed(() => ({
  tooltip: { trigger: 'item' },
  series: [
    {
      type: 'sankey',
      layout: 'none',
      nodeAlign: 'justify',
      emphasis: { focus: 'adjacency' },
      data: props.nodes.map((node) => ({
        name: node.id,
        label: { formatter: `${node.label} (${node.count})` },
        value: node.count,
      })),
      links: props.edges.map((edge) => ({
        source: edge.source,
        target: edge.target,
        value: edge.count,
      })),
    },
  ],
}))
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Learning flow</p>
        <h2>Last 7 days</h2>
      </div>
    </header>
    <div v-if="nodes.length === 0" class="chart-empty">No learning flow data</div>
    <VChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
      @click="(event: any) => emit('selectNode', String(event.name ?? ''))"
    />
  </section>
</template>
```

- [ ] **Step 2: Create LearningSignalPanel**

Create `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue`:

```vue
<!-- crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue -->
<script setup lang="ts">
import StatusPill from '../StatusPill.vue'
import { shortDate } from '../../format'
import type { LearningSignalPoint } from '../../types'

defineProps<{
  signals: LearningSignalPoint[]
}>()
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Learning signals</p>
        <h2>Recent outcomes</h2>
      </div>
    </header>
    <p v-if="signals.length === 0" class="muted-line">No recent learning outcomes</p>
    <div v-else class="row-list">
      <div v-for="signal in signals" :key="signal.id" class="data-row static">
        <span class="row-main">
          <strong>{{ signal.label }}</strong>
          <small>{{ shortDate(signal.occurred_at) }}</small>
        </span>
        <span class="row-side">
          <StatusPill :status="signal.severity" />
        </span>
      </div>
    </div>
  </aside>
</template>
```

- [ ] **Step 3: Wire ReportsView**

In `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`, import the new components and render them before metric cards:

```vue
<!-- crates/right-dashboard/frontend/src/views/learning/ReportsView.vue -->
<script setup lang="ts">
import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import MetricCard from '../../components/MetricCard.vue'
import StatusPill from '../../components/StatusPill.vue'
import { percent, shortDate } from '../../format'
import type { LearningOverviewResponse, LearningReportDetailResponse, LearningReportSummary } from '../../types'

// keep existing props and emits
</script>
```

Add this near the top of the template:

```vue
<!-- crates/right-dashboard/frontend/src/views/learning/ReportsView.vue -->
<section v-if="learning?.warnings.length" class="notice">
  <strong>Partial data</strong>
  <span>{{ learning.warnings[0].message }}</span>
</section>

<section class="two-column wide-main">
  <LearningFlowChart
    :nodes="learning?.flow_nodes ?? []"
    :edges="learning?.flow_edges ?? []"
  />
  <LearningSignalPanel :signals="learning?.recent_learning_signals ?? []" />
</section>
```

Keep the existing metrics and report/detail list after the chart section.

- [ ] **Step 4: Run frontend typecheck**

Run:

```bash
cd crates/right-dashboard/frontend
npm run typecheck
```

Expected: PASS.

- [ ] **Step 5: Commit learning frontend**

```bash
git add crates/right-dashboard/frontend/src/components/charts/LearningFlowChart.vue crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue crates/right-dashboard/frontend/src/views/learning/ReportsView.vue
git commit -m "feat(dashboard): render learning flow"
```

## Task 10: Build Assets And Documentation

**Files:**
- Modify: `crates/right-dashboard/static/dashboard/**`
- Modify if drifted: `docs/architecture/modules.md`
- Modify if needed: `ARCHITECTURE.md`

- [ ] **Step 1: Build dashboard static assets**

Run:

```bash
cd crates/right-dashboard/frontend
npm run build
```

Expected: PASS and files under `crates/right-dashboard/static/dashboard/` update.

- [ ] **Step 2: Run targeted Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_overview usage_overview learning_overview
devenv shell -- cargo test -p right-bot dashboard
```

Expected: PASS.

- [ ] **Step 3: Update architecture module docs if drifted**

Read the dashboard entry:

```bash
devenv shell -- sed -n '60,105p' docs/architecture/modules.md
```

If it does not mention visual analytics/chart components after this implementation, update the `right-dashboard` bullets to include:

```markdown
- `frontend/src/components/charts/` — Vue/ECharts components for overview signal timeline, cost/learning river, usage spend chart, and learning flow.
```

If `ARCHITECTURE.md` still accurately states that `right-dashboard` owns read-only overview/activity/knowledge/usage DTOs and assets, leave it unchanged. If it is missing the read-only visual analytics contract after implementation, add one concise sentence to the existing `right-dashboard` paragraph.

- [ ] **Step 4: Commit assets and docs**

```bash
git add crates/right-dashboard/static/dashboard docs/architecture/modules.md ARCHITECTURE.md
git commit -m "docs(dashboard): document visual analytics assets"
```

If only static assets changed, use:

```bash
git add crates/right-dashboard/static/dashboard
git commit -m "build(dashboard): refresh static assets"
```

## Task 11: Final Verification

**Files:**
- Verify entire workspace.

- [ ] **Step 1: Run frontend build one final time**

Run:

```bash
cd crates/right-dashboard/frontend
npm run build
```

Expected: PASS.

- [ ] **Step 2: Run full mandatory workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Check git status**

Run:

```bash
devenv shell -- git status --short
```

Expected: clean worktree or only intentional uncommitted changes explicitly called out.

- [ ] **Step 4: Final implementation summary**

Report:

```text
Implemented dashboard visual analytics:
- overview signal timeline + cost/learning river
- usage spend-over-time chart
- knowledge learning-flow chart
- read-only DTO/read-model backing fields

Verification:
- npm run build
- cargo test --workspace
```

## Plan Self-Review

- Spec coverage: Overview signal timeline and cost/learning river are covered in Tasks 1, 3, and 7. Usage spend-over-time is covered in Tasks 1, 2, 6, and 8. Knowledge learning flow is covered in Tasks 1, 4, 6, and 9. Chart dependency and hover/select-only behavior are covered in Tasks 6-9. Read-only route/auth boundary is preserved and regression-tested in Task 5.
- Placeholder scan: implementation steps contain concrete paths, commands, expected results, and code blocks rather than deferred work markers.
- Type consistency: Rust DTO names and TypeScript interface names match: `DashboardSignal`, `CostLearningRiver`, `UsageDailyPoint`, `UsageSourcePoint`, `LearningFlowNode`, `LearningFlowEdge`, `LearningSignalPoint`, and `DashboardDataWarning`.
- Verification cadence: baseline targeted checks happen first, each implementation slice has targeted tests/typecheck, frontend build happens after chart work, and the final full workspace test is mandatory.
