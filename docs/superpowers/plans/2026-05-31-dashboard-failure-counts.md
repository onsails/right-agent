# Dashboard Failure Counts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every dashboard failure count render neutral gray at zero and, when non-zero, expand into the complete untruncated list of those failures.

**Architecture:** Three read-model responses each gain an untruncated failure-list field whose length equals its existing count (Overview failed `async_runs` 24h, Activity failed cron runs 7d, Reports failed/aborted learning events 7d). The frontend adds a pure `failureMetric()` helper (tone + interactivity), makes `MetricCard` optionally clickable, adds a controlled `open` to `CollapsibleSection`, and a shared `RunFailureList` that expands per-row error+log via the existing `runDetail(id)` endpoint. Each failure `MetricCard` toggles a `CollapsibleSection` holding its list.

**Tech Stack:** Rust (edition 2024, `right-db`/turso async read models), Vue 3 `<script setup>` + TypeScript, Vitest + `@vue/server-renderer` (SSR-only tests, no DOM events), pnpm.

**Source spec:** `docs/superpowers/specs/2026-05-31-dashboard-failure-counts-design.md`

**Refinement vs spec §backend-2:** Overview failure rows are plain `RunSummary` (no inline `error_json` parsing). Error + log surface via the shared `RunFailureList` row-click → `runDetail(id)`, exactly like Activity. The `extract_error_message` extractor already runs inside `activity_run_detail`, so no list-builder parsing is needed.

---

## File Structure

**Backend — `crates/right-dashboard/src/`**
- `read_model/run_summary.rs` *(new)* — shared `RunSummary` SQL column list, FROM clause, and row mapper; hoisted out of `activity.rs` so both activity and overview read models build identical rows.
- `read_model.rs` *(modify)* — declare `mod run_summary;`.
- `read_model/activity.rs` *(modify)* — import the hoisted helpers; add `failed_cron_runs()`; recompute `failed_recent_cron_count`.
- `read_model/dashboard_overview.rs` *(modify)* — add `recent_failed_runs()`.
- `read_model/learning.rs` *(modify)* — add untruncated `recent_failed_events()`.
- `api_types.rs` *(modify)* — three new response fields.

**Frontend — `crates/right-dashboard/frontend/src/`**
- `components/failureMetric.ts` *(new)* + `failureMetric.test.ts` *(new)* — pure tone/interactivity decision.
- `components/MetricCard.vue` *(modify)* + `MetricCard.test.ts` *(new)* — optional interactive `<button>` + `select` emit.
- `components/CollapsibleSection.vue` *(modify)* + `CollapsibleSection.test.ts` *(extend)* — controlled `open` (`v-model:open`).
- `components/RunFailureList.vue` *(new)* + `RunFailureList.test.ts` *(new)* — shared failed-run rows with inline detail.
- `types.ts` *(modify)* — mirror the three new fields.
- `views/OverviewView.vue`, `views/ActivityView.vue`, `views/learning/ReportsView.vue` *(modify)* — wire card → section.

**Verification cadence:** targeted `devenv shell -- cargo test -p right-dashboard <filter>` and `pnpm test <file>` per task; one mandatory `devenv shell -- cargo test --workspace` + `pnpm test` + `pnpm build` at the end (Task 12). Do not run the full workspace suite between every task.

---

## Task 1: Hoist shared `RunSummary` query helpers (refactor)

Pure move so `dashboard_overview.rs` can build the same `RunSummary` rows as `activity.rs`. No behavior change — existing activity tests are the safety net.

**Files:**
- Create: `crates/right-dashboard/src/read_model/run_summary.rs`
- Modify: `crates/right-dashboard/src/read_model.rs` (add `mod run_summary;`)
- Modify: `crates/right-dashboard/src/read_model/activity.rs` (remove moved items; import them)

- [ ] **Step 1: Confirm `delivery_kind_from_json` is only used by the row mapper**

Run: `rg -n "delivery_kind_from_json" crates/right-dashboard/src`
Expected: matches only inside `activity.rs` (its definition + use in `run_summary_from_row`). If used elsewhere, leave a `pub(crate)` re-export — otherwise it moves wholesale.

- [ ] **Step 2: Create the shared module**

Create `crates/right-dashboard/src/read_model/run_summary.rs`:

```rust
//! Shared async-run → `RunSummary` query fragments, reused by the activity
//! and overview read models so failed-run lists render identical rows.

use crate::api_types::RunSummary;

pub(crate) const RUN_SUMMARY_COLUMNS: &str =
    "ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
        ar.exit_code, ar.delivery_status, ar.delivery_required, ar.delivery_json,
        ar.run_note, costs.cost_usd";

pub(crate) const RUN_SUMMARY_FROM: &str = "FROM async_runs ar
 LEFT JOIN (
    SELECT session_uuid, SUM(total_cost_usd) AS cost_usd
    FROM usage_events
    GROUP BY session_uuid
 ) costs ON costs.session_uuid = ar.run_session_id";

pub(crate) fn run_summary_from_row(
    row: &right_db::row::Row<'_>,
) -> Result<RunSummary, right_db::DbError> {
    Ok(RunSummary {
        id: row.get(0)?,
        kind: row.get(1)?,
        producer_ref: row.get(2)?,
        status: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        exit_code: row.get(6)?,
        delivery_status: row.get(7)?,
        delivery_required: row.get::<_, i64>(8)? != 0,
        delivery_kind: delivery_kind_from_json(row.get::<_, Option<String>>(9)?.as_deref()),
        run_note: row.get(10)?,
        cost_usd: row.get(11)?,
    })
}

fn delivery_kind_from_json(json: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(json?).ok()?;
    value.get("kind")?.as_str().map(ToOwned::to_owned)
}
```

- [ ] **Step 3: Declare the module**

In `crates/right-dashboard/src/read_model.rs`, add alongside the other `mod` lines (after `pub mod dashboard_overview;`):

```rust
mod run_summary;
```

- [ ] **Step 4: Remove the moved items from `activity.rs` and import them**

In `crates/right-dashboard/src/read_model/activity.rs`: delete the `RUN_SUMMARY_COLUMNS` const, the `RUN_SUMMARY_FROM` const, the `run_summary_from_row` fn, and the `delivery_kind_from_json` fn. Add this import near the top (next to `use super::ReadModelError;`):

```rust
use super::run_summary::{RUN_SUMMARY_COLUMNS, RUN_SUMMARY_FROM, run_summary_from_row};
```

- [ ] **Step 5: Build + run existing activity tests (must stay green)**

Run: `devenv shell -- cargo test -p right-dashboard activity`
Expected: compiles; all existing `activity_*` tests PASS (no behavior change).

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/read_model/run_summary.rs crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/activity.rs
git commit -m "refactor(dashboard): hoist shared RunSummary query helpers"
```

---

## Task 2: Overview `recent_failed_runs` (24h)

Add an untruncated list of failed `async_runs` (all kinds) over the same 24h window as `recent_failures`, so `recent_failed_runs.len() == recent_failures`.

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs` (`DashboardOverviewResponse`)
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs`
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`DashboardOverviewResponse`)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` in `crates/right-dashboard/src/read_model/dashboard_overview.rs` (reuse the existing test harness/fixtures in that module for seeding `async_runs`; mirror the seeding used by the existing `recent_failures` test):

```rust
#[tokio::test]
async fn recent_failed_runs_matches_failure_count_and_lists_each_run() {
    let conn = setup_overview_db().await; // existing helper used by recent_failures tests
    seed_failed_async_run(&conn, "run-f1", "cron", "2026-05-31T11:00:00Z").await;
    seed_failed_async_run(&conn, "run-f2", "background", "2026-05-31T11:30:00Z").await;

    let response = dashboard_overview(&conn, overview_input("2026-05-31T12:00:00Z")).await.unwrap();

    assert_eq!(response.recent_failures, 2);
    assert_eq!(response.recent_failed_runs.len(), 2);
    assert_eq!(response.recent_failed_runs.len() as i64, response.recent_failures);
    // newest first
    assert_eq!(response.recent_failed_runs[0].id, "run-f2");
    assert_eq!(response.recent_failed_runs[1].id, "run-f1");
}
```

> If `setup_overview_db` / `seed_failed_async_run` / `overview_input` are named differently in this module, reuse whatever the existing `recent_failures` test already calls — do not invent new fixtures.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard recent_failed_runs_matches_failure_count`
Expected: FAIL — `no field recent_failed_runs on DashboardOverviewResponse`.

- [ ] **Step 3: Add the response field (Rust + TS)**

In `crates/right-dashboard/src/api_types.rs`, add to `DashboardOverviewResponse` (after `recent_failures`):

```rust
    pub recent_failed_runs: Vec<RunSummary>,
```

(`RunSummary` is defined in this file — no import needed.)

In `crates/right-dashboard/frontend/src/types.ts`, add to `interface DashboardOverviewResponse` (after `recent_failures: number`):

```ts
  recent_failed_runs: RunSummary[]
```

(`RunSummary` already exists in `types.ts`.)

- [ ] **Step 4: Add the read-model query and populate the field**

In `crates/right-dashboard/src/read_model/dashboard_overview.rs`, add the import for the hoisted helpers near the top imports:

```rust
use super::run_summary::{RUN_SUMMARY_COLUMNS, RUN_SUMMARY_FROM, run_summary_from_row};
```

Add `RunSummary` to the existing `use crate::api_types::{...}` import. Then add this function next to `recent_failure_count`:

```rust
async fn recent_failed_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<Vec<RunSummary>, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::hours(24);
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(&since, &now);
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS},
                COALESCE(ar.finished_at, ar.updated_at, ar.created_at) AS win_ts
         {RUN_SUMMARY_FROM}
         WHERE ar.status = 'failed'
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) >= ?1
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) <= ?2
         ORDER BY COALESCE(ar.finished_at, ar.updated_at, ar.created_at) DESC, ar.created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until], |row| {
            Ok((run_summary_from_row(row)?, row.get::<_, String>(12)?))
        })
        .await?;
    // Precise window filter mirrors recent_failure_count so list length == count.
    let mut out = Vec::new();
    for row in rows {
        let (run, win_ts) = row?;
        let ts = parse_utc(&win_ts)?;
        if ts >= since && ts <= now {
            out.push(run);
        }
    }
    Ok(out)
}
```

In the `dashboard_overview(...)` builder, compute it alongside `recent_failures` and set the field:

```rust
    let recent_failed_runs = recent_failed_runs(conn, &input.generated_at).await?;
```

and add `recent_failed_runs,` to the `DashboardOverviewResponse { ... }` literal.

> The `win_ts` alias is column index 12 (the 12 `RUN_SUMMARY_COLUMNS` are indices 0–11). `parse_utc`, `coarse_timestamp_bounds`, `Duration`, `params!` are already in scope in this module.

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-dashboard recent_failed_runs_matches_failure_count`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/dashboard_overview.rs crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): overview recent_failed_runs list (24h)"
```

---

## Task 3: Activity `failed_runs` + count recompute (7d)

Replace the "crons with a failed last-5 run" count with the number of failed cron runs in 7d, and expose those runs as a flat list. `failed_recent_cron_count == failed_runs.len()`.

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs` (`OverviewResponse`)
- Modify: `crates/right-dashboard/src/read_model/activity.rs`
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`OverviewResponse`)

- [ ] **Step 1: Write the failing test + update the stale one**

In `crates/right-dashboard/src/read_model/activity.rs` tests: locate the existing assertion on `failed_recent_cron_count` and update its expected value to the count of failed **cron runs** in the 7d window (not crons). Then add:

```rust
#[tokio::test]
async fn failed_runs_lists_failed_cron_runs_in_window_and_matches_count() {
    let conn = setup_activity_db().await; // existing helper used by activity_overview tests
    seed_cron_run(&conn, "run-1", "job-a", "failed", "2026-05-31T10:00:00Z").await;
    seed_cron_run(&conn, "run-2", "job-a", "failed", "2026-05-31T11:00:00Z").await;
    seed_cron_run(&conn, "run-3", "job-b", "completed", "2026-05-31T11:30:00Z").await;

    let response = activity_overview(&conn, activity_input("2026-05-31T12:00:00Z")).await.unwrap();

    assert_eq!(response.failed_runs.len(), 2);
    assert_eq!(response.summary.failed_recent_cron_count, response.failed_runs.len());
    assert_eq!(response.failed_runs[0].id, "run-2"); // newest first
}
```

> Reuse the existing activity-test seeding helpers/names; do not invent new ones. If runs are seeded with explicit `INSERT INTO async_runs (...)`, follow that exact pattern with `kind='cron'` and `producer_ref=<job_name>`.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard failed_runs_lists_failed_cron_runs`
Expected: FAIL — `no field failed_runs on OverviewResponse`.

- [ ] **Step 3: Add the response field (Rust + TS)**

In `crates/right-dashboard/src/api_types.rs`, add to `OverviewResponse` (after `crons`):

```rust
    pub failed_runs: Vec<RunSummary>,
```

In `crates/right-dashboard/frontend/src/types.ts`, add to `interface OverviewResponse` (after `crons: CronCard[]`):

```ts
  failed_runs: RunSummary[]
```

- [ ] **Step 4: Query failed cron runs; recompute the count**

In `crates/right-dashboard/src/read_model/activity.rs`, ensure these are imported (add any missing): `use super::{parse_utc, coarse_timestamp_bounds};` (they are `pub(crate)` in `read_model.rs`) and `use chrono::Duration;`. Add the function:

```rust
async fn failed_cron_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<Vec<RunSummary>, ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::days(7);
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(&since, &now);
    let sql = format!(
        "SELECT {RUN_SUMMARY_COLUMNS},
                COALESCE(ar.finished_at, ar.updated_at, ar.created_at) AS win_ts
         {RUN_SUMMARY_FROM}
         WHERE ar.kind = 'cron' AND ar.status = 'failed'
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) >= ?1
           AND COALESCE(ar.finished_at, ar.updated_at, ar.created_at) <= ?2
         ORDER BY COALESCE(ar.finished_at, ar.updated_at, ar.created_at) DESC, ar.created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![coarse_since, coarse_until], |row| {
            Ok((run_summary_from_row(row)?, row.get::<_, String>(12)?))
        })
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let (run, win_ts) = row?;
        let ts = parse_utc(&win_ts)?;
        if ts >= since && ts <= now {
            out.push(run);
        }
    }
    Ok(out)
}
```

In `activity_overview(...)`: delete the old `let failed_recent_cron_count = crons.iter().filter(...).count();` block. Add:

```rust
    let failed_runs = failed_cron_runs(conn, &input.generated_at).await?;
    let failed_recent_cron_count = failed_runs.len();
```

Add `failed_runs,` to the `OverviewResponse { ... }` literal (the `failed_recent_cron_count` already feeds `OverviewSummary`).

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-dashboard activity`
Expected: PASS — the new test and the updated `failed_recent_cron_count` assertion both green.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/activity.rs crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): activity failed_runs list + run-based count (7d)"
```

---

## Task 4: Reports `recent_failed_events` (7d, untruncated)

Mirror `recent_successful_events` but for `status IN ('failed','aborted')` and **without** `RECENT_EVENT_LIMIT` truncation, so `recent_failed_events.len() == failed_or_aborted_7d`.

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs` (`LearningLifecycle`)
- Modify: `crates/right-dashboard/src/read_model/learning.rs`
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`LearningLifecycle`)

- [ ] **Step 1: Write the failing test**

In `crates/right-dashboard/src/read_model/learning.rs` tests, add (reuse the existing learning-test seeding for `skill_learning_events`):

```rust
#[tokio::test]
async fn recent_failed_events_includes_failed_and_aborted_untruncated() {
    let conn = setup_learning_db().await; // existing helper
    // Seed more than RECENT_EVENT_LIMIT (10) failed/aborted finish events.
    for i in 0..12 {
        seed_finish_event(&conn, "agent", &format!("skill-{i}"), "failed",
            &format!("2026-05-3{}T10:00:00Z", i % 2)).await;
    }
    seed_finish_event(&conn, "agent", "skill-ab", "aborted", "2026-05-31T10:00:00Z").await;

    let response = skill_lifecycle_overview(&conn, "agent", overview_input("2026-05-31T12:00:00Z")).await.unwrap();

    assert_eq!(response.lifecycle.recent_failed_events.len() as i64, response.lifecycle.failed_or_aborted_7d);
    assert!(response.lifecycle.recent_failed_events.len() >= 13); // not truncated at 10
    assert!(response.lifecycle.recent_failed_events.iter().any(|e| e.status == "aborted"));
    assert!(response.lifecycle.recent_failed_events.iter().all(|e| e.status == "failed" || e.status == "aborted"));
}
```

> Use whatever the existing learning tests call to insert finish events and to invoke the lifecycle builder; match their signatures exactly.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard recent_failed_events_includes_failed_and_aborted`
Expected: FAIL — `no field recent_failed_events on LearningLifecycle`.

- [ ] **Step 3: Add the field (Rust + TS)**

In `crates/right-dashboard/src/api_types.rs`, add to `LearningLifecycle` (after `recent_successful_events`):

```rust
    pub recent_failed_events: Vec<LearningEventSummary>,
```

In `crates/right-dashboard/frontend/src/types.ts`, add to `interface LearningLifecycle` (after `recent_successful_events: LearningEventSummary[]`):

```ts
  recent_failed_events: LearningEventSummary[]
```

- [ ] **Step 4: Add the query and populate**

In `crates/right-dashboard/src/read_model/learning.rs`, add the function (identical to `recent_successful_events` except the status filter and **no `truncate`**):

```rust
async fn recent_failed_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let (coarse_since, coarse_until) = coarse_timestamp_bounds(since_7d, now);
    let mut stmt = conn.prepare(
        "SELECT id, skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('failed','aborted')
           AND created_at >= ?2
           AND created_at <= ?3
         ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map(params![agent, coarse_since, coarse_until], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .await?;
    let mut events = Vec::<(DateTime<Utc>, i64, LearningEventSummary)>::new();
    for row in rows {
        let (id, skill_name, action, status, message, summary, created_at) = row?;
        let created_at_utc = parse_utc(&created_at)?;
        if created_at_utc < *since_7d || created_at_utc > *now {
            continue;
        }
        events.push((
            created_at_utc,
            id,
            LearningEventSummary { skill_name, action, status, message, summary, created_at: created_at_utc.to_rfc3339() },
        ));
    }
    events.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    // NOTE: intentionally not truncated — the list must equal failed_or_aborted_7d.
    Ok(events.into_iter().map(|(_, _, event)| event).collect())
}
```

In the `LearningLifecycle { ... }` literal inside `skill_lifecycle_overview(...)`, add after the `recent_successful_events:` line:

```rust
        recent_failed_events: recent_failed_events(conn, agent, since_7d, now).await?,
```

> If any existing test constructs a whole `LearningLifecycle { ... }` literal, add `recent_failed_events: vec![]` there — `cargo test -p right-dashboard learning` surfaces it.

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-dashboard learning`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/learning.rs crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): learning recent_failed_events list (untruncated, 7d)"
```

---

## Task 5: `failureMetric` helper (frontend, pure)

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/failureMetric.ts`
- Test: `crates/right-dashboard/frontend/src/components/failureMetric.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/failureMetric.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { failureMetric } from './failureMetric'

describe('failureMetric', () => {
  it('is gray and inert at zero', () => {
    expect(failureMetric(0)).toEqual({ tone: 'default', interactive: false })
  })
  it('is red and interactive when positive', () => {
    expect(failureMetric(3)).toEqual({ tone: 'bad', interactive: true })
  })
  it('treats negative counts as zero-like', () => {
    expect(failureMetric(-1)).toEqual({ tone: 'default', interactive: false })
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/failureMetric.test.ts`
Expected: FAIL — cannot resolve `./failureMetric`.

- [ ] **Step 3: Implement the helper**

Create `crates/right-dashboard/frontend/src/components/failureMetric.ts`:

```ts
export interface FailureMetric {
  tone: 'default' | 'bad'
  interactive: boolean
}

/** A failure count is calm (gray, inert) at zero and red+clickable above it. */
export function failureMetric(count: number): FailureMetric {
  if (count > 0) {
    return { tone: 'bad', interactive: true }
  }
  return { tone: 'default', interactive: false }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/failureMetric.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/failureMetric.ts crates/right-dashboard/frontend/src/components/failureMetric.test.ts
git commit -m "feat(dashboard): failureMetric tone/interactivity helper"
```

---

## Task 6: `MetricCard` optional interactivity

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/MetricCard.vue`
- Test: `crates/right-dashboard/frontend/src/components/MetricCard.test.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/MetricCard.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import MetricCard from './MetricCard.vue'

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(MetricCard, props as any),
  })
  return renderToString(app)
}

describe('MetricCard', () => {
  it('renders a static article by default', async () => {
    const html = await render({ label: 'Failures', value: 0, tone: 'default' })
    expect(html).toContain('<article')
    expect(html).not.toContain('<button')
  })
  it('renders a clickable button when interactive', async () => {
    const html = await render({ label: 'Failures', value: 3, tone: 'bad', interactive: true })
    expect(html).toContain('<button')
    expect(html).toContain('Failures')
    expect(html).toContain('3')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/MetricCard.test.ts`
Expected: FAIL — interactive case has no `<button>` (current template is always `<article>`).

- [ ] **Step 3: Implement interactivity**

Replace the entire contents of `crates/right-dashboard/frontend/src/components/MetricCard.vue` with:

```vue
<script setup lang="ts">
defineProps<{
  label: string
  value: string | number
  tone?: 'default' | 'ok' | 'active' | 'bad'
  interactive?: boolean
}>()

defineEmits<{
  select: []
}>()
</script>

<template>
  <button
    v-if="interactive"
    type="button"
    class="metric-card metric-card-interactive"
    :class="tone ?? 'default'"
    @click="$emit('select')"
  >
    <span>{{ label }}</span>
    <strong>{{ value }}</strong>
  </button>
  <article v-else class="metric-card" :class="tone ?? 'default'">
    <span>{{ label }}</span>
    <strong>{{ value }}</strong>
  </article>
</template>

<style scoped>
.metric-card-interactive {
  display: block;
  width: 100%;
  font: inherit;
  text-align: left;
  cursor: pointer;
  color: inherit;
}
.metric-card-interactive:hover,
.metric-card-interactive:focus-visible {
  border-color: var(--tg-theme-link-color, #2f6feb);
}
.metric-card-interactive strong::after {
  content: ' ›';
  color: var(--tg-theme-hint-color, #6b7b88);
}
</style>
```

> Base `.metric-card` / `.metric-card.bad` styling stays global in `App.vue`. The scoped block only normalizes the `<button>` and adds the affordance.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/MetricCard.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/MetricCard.vue crates/right-dashboard/frontend/src/components/MetricCard.test.ts
git commit -m "feat(dashboard): optional clickable MetricCard"
```

---

## Task 7: `CollapsibleSection` controlled `open`

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue`
- Test: `crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts` (extend)

- [ ] **Step 1: Write the failing test (extend existing)**

Append two cases inside the existing `describe('CollapsibleSection', ...)` in `crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts`:

```ts
  it('uses the controlled open prop when provided', async () => {
    const html = await render({ title: 'core', count: 3, open: true })
    expect(html).toContain('BODY')
  })
  it('controlled open=false overrides defaultOpen', async () => {
    const html = await render({ title: 'core', count: 3, defaultOpen: true, open: false })
    expect(html).not.toContain('BODY')
  })
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/CollapsibleSection.test.ts`
Expected: FAIL — controlled `open` is ignored (body visibility still driven by internal `defaultOpen`).

- [ ] **Step 3: Implement controlled open**

Replace the `<script setup>` and `<template>` of `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue` (keep the existing `<style scoped>` block unchanged):

```vue
<script setup lang="ts">
import { computed, ref } from 'vue'

const props = withDefaults(defineProps<{
  title: string
  count: number
  defaultOpen?: boolean
  open?: boolean
}>(), { defaultOpen: false, open: undefined })

const emit = defineEmits<{
  'update:open': [value: boolean]
}>()

const internalOpen = ref(props.defaultOpen)
const isOpen = computed(() => props.open ?? internalOpen.value)

function toggle(): void {
  const next = !isOpen.value
  internalOpen.value = next
  emit('update:open', next)
}
</script>

<template>
  <article class="panel collapsible">
    <button
      type="button"
      class="panel-head collapsible-head"
      :aria-expanded="isOpen"
      @click="toggle"
    >
      <span class="collapsible-title">
        <span class="chevron" :class="{ open: isOpen }" aria-hidden="true">›</span>
        <strong>{{ title }}</strong>
        <span class="count-badge">{{ count }}</span>
      </span>
    </button>
    <div v-if="isOpen" class="collapsible-body">
      <slot />
    </div>
  </article>
</template>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/CollapsibleSection.test.ts`
Expected: PASS — original 3 + new 2 = 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/CollapsibleSection.vue crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts
git commit -m "feat(dashboard): controlled open prop on CollapsibleSection"
```

---

## Task 8: `RunFailureList` shared component

Renders failed `RunSummary` rows; clicking a row fetches `runDetail(id)` and expands error+log inline. (SSR tests cover row rendering + empty state; click→detail is exercised manually — `renderToString` has no events.)

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/RunFailureList.vue`
- Test: `crates/right-dashboard/frontend/src/components/RunFailureList.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/RunFailureList.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it, vi } from 'vitest'

import RunFailureList from './RunFailureList.vue'

vi.mock('../api', () => ({ runDetail: vi.fn() }))

function failedRun(id: string) {
  return {
    id,
    kind: 'cron',
    producer_ref: 'job-x',
    status: 'failed',
    started_at: null,
    finished_at: '2026-05-31T11:00:00Z',
    exit_code: 1,
    delivery_required: false,
    delivery_status: 'none',
    delivery_kind: null,
    run_note: null,
    cost_usd: 0.12,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(RunFailureList, props as any),
  })
  return renderToString(app)
}

describe('RunFailureList', () => {
  it('renders one row per failed run', async () => {
    const html = await render({ runs: [failedRun('run-aaaaaaaa'), failedRun('run-bbbbbbbb')] })
    expect(html).toContain('cron')
    expect(html).toContain('job-x')
  })
  it('shows an empty hint when there are no runs', async () => {
    const html = await render({ runs: [] })
    expect(html).toContain('No failures')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/RunFailureList.test.ts`
Expected: FAIL — cannot resolve `./RunFailureList.vue`.

- [ ] **Step 3: Implement the component**

Create `crates/right-dashboard/frontend/src/components/RunFailureList.vue`:

```vue
<script setup lang="ts">
import { ref } from 'vue'

import { runDetail } from '../api'
import { money, shortDate, shortId, statusTone } from '../format'
import type { RunDetailResponse, RunSummary } from '../types'
import AsyncState from './AsyncState.vue'

defineProps<{
  runs: RunSummary[]
}>()

const selectedId = ref<string | null>(null)
const detail = ref<RunDetailResponse | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)

async function select(run: RunSummary): Promise<void> {
  if (selectedId.value === run.id) {
    selectedId.value = null
    detail.value = null
    return
  }
  selectedId.value = run.id
  detail.value = null
  error.value = null
  loading.value = true
  try {
    const result = await runDetail(run.id)
    if (selectedId.value === run.id) {
      detail.value = result
    }
  } catch (err) {
    if (selectedId.value === run.id) {
      error.value = err instanceof Error ? err.message : 'Failed to load run detail'
    }
  } finally {
    if (selectedId.value === run.id) {
      loading.value = false
    }
  }
}
</script>

<template>
  <p v-if="runs.length === 0" class="muted-line">No failures</p>
  <div v-else class="row-list">
    <template v-for="run in runs" :key="run.id">
      <button
        class="data-row"
        :class="{ selected: selectedId === run.id }"
        type="button"
        @click="select(run)"
      >
        <span class="row-main">
          <span class="status-dot" :class="statusTone(run.status)"></span>
          <strong>{{ run.kind }}</strong>
          <small>{{ shortId(run.id) }}</small>
          <small v-if="run.producer_ref">{{ run.producer_ref }}</small>
        </span>
        <span class="row-side">
          <strong>{{ money(run.cost_usd) }}</strong>
          <small>{{ shortDate(run.finished_at ?? run.started_at) }}</small>
        </span>
      </button>

      <section v-if="selectedId === run.id" class="run-inline-detail">
        <AsyncState
          :loading="loading"
          :error="error"
          :empty="!detail || detail.run.id !== run.id"
          empty-text="No run detail"
        >
          <dl class="meta-grid compact">
            <div>
              <dt>Exit</dt>
              <dd>{{ detail!.run.exit_code ?? 'none' }}</dd>
            </div>
            <div>
              <dt>Finished</dt>
              <dd>{{ shortDate(detail!.run.finished_at) }}</dd>
            </div>
          </dl>
          <section v-if="detail!.error_message" class="text-block">
            <h3>Error</h3>
            <p>{{ detail!.error_message }}</p>
          </section>
          <section class="text-block">
            <h3>Log</h3>
            <p v-if="!detail!.log.available" class="muted-line">Log unavailable</p>
            <pre v-else>{{ detail!.log.lines.join('\n') }}<template v-if="detail!.log.truncated">
... truncated
</template></pre>
          </section>
        </AsyncState>
      </section>
    </template>
  </div>
</template>
```

> `money`, `shortDate`, `shortId`, `statusTone` exist in `format.ts`; `data-row`, `row-main`, `status-dot`, `run-inline-detail`, `text-block`, `meta-grid compact` are global classes in `App.vue` (same ones ActivityView uses).

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/components/RunFailureList.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/RunFailureList.vue crates/right-dashboard/frontend/src/components/RunFailureList.test.ts
git commit -m "feat(dashboard): shared RunFailureList with inline run detail"
```

---

## Task 9: Wire `OverviewView`

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue`
- Test: `crates/right-dashboard/frontend/src/views/OverviewView.test.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/OverviewView.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import OverviewView from './OverviewView.vue'

function overview(recentFailures: number, failedRuns: unknown[]) {
  return {
    agent: 'a', generated_at: '2026-05-31T12:00:00Z',
    active_runs: 0, recent_failures: recentFailures, today_cost_usd: 0,
    learning_candidates_24h: 0,
    doctor: { state: 'ok', pass_count: 1, warn_count: 0, fail_count: 0, generated_at: null },
    sandbox: { state: 'ok', detail: null },
    signals: [], cost_learning_river: null, warnings: [],
    recent_failed_runs: failedRuns,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(OverviewView, props as any),
  })
  return renderToString(app)
}

describe('OverviewView failures card', () => {
  it('renders the zero failures card without the bad tone', async () => {
    const html = await render({ overview: overview(0, []), activity: null, loading: false, error: null })
    expect(html).toContain('Failures')
    // gray card is a static <article class="metric-card default">, never bad at zero
    expect(html).not.toMatch(/metric-card[^"]*\bbad\b/)
  })
  it('marks the failures card bad when non-zero', async () => {
    const failed = [{ id: 'run-1', kind: 'cron', producer_ref: 'job', status: 'failed', started_at: null, finished_at: '2026-05-31T11:00:00Z', exit_code: 1, delivery_required: false, delivery_status: 'none', delivery_kind: null, run_note: null, cost_usd: 0 }]
    const html = await render({ overview: overview(1, failed), activity: null, loading: false, error: null })
    expect(html).toMatch(/metric-card[^"]*\bbad\b/)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/OverviewView.test.ts`
Expected: FAIL — at zero the current template forces `tone="ok"`/`bad`, so the assertions don't hold yet.

- [ ] **Step 3: Wire the card + section**

In `crates/right-dashboard/frontend/src/views/OverviewView.vue` `<script setup>`, add imports:

```ts
import CollapsibleSection from '../components/CollapsibleSection.vue'
import RunFailureList from '../components/RunFailureList.vue'
import { failureMetric } from '../components/failureMetric'
```

Add state (after the existing `selectedMarkerId` ref; `computed`/`ref` are already imported):

```ts
const failuresOpen = ref(false)
const failures = computed(() => failureMetric(props.overview?.recent_failures ?? 0))
```

Replace the existing Failures `MetricCard` line:

```vue
      <MetricCard label="Failures" :value="overview?.recent_failures ?? 0" :tone="(overview?.recent_failures ?? 0) > 0 ? 'bad' : 'ok'" />
```

with:

```vue
      <MetricCard
        label="Failures"
        :value="overview?.recent_failures ?? 0"
        :tone="failures.tone"
        :interactive="failures.interactive"
        @select="failuresOpen = !failuresOpen"
      />
```

Immediately after the closing `</section>` of `<section class="metric-grid">`, add:

```vue
    <CollapsibleSection
      v-if="(overview?.recent_failures ?? 0) > 0"
      v-model:open="failuresOpen"
      title="Failures"
      :count="overview?.recent_failures ?? 0"
    >
      <RunFailureList :runs="overview?.recent_failed_runs ?? []" />
    </CollapsibleSection>
```

- [ ] **Step 4: Run test + type-check**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/OverviewView.test.ts && devenv shell -- pnpm build`
Expected: tests PASS; `vue-tsc` type-check succeeds (confirms `recent_failed_runs` binds correctly).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/OverviewView.vue crates/right-dashboard/frontend/src/views/OverviewView.test.ts
git commit -m "feat(dashboard): wire Overview failures card to failure list"
```

---

## Task 10: Wire `ActivityView`

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- Test: `crates/right-dashboard/frontend/src/views/ActivityView.test.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/ActivityView.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ActivityView from './ActivityView.vue'

function activity(failedCount: number, failedRuns: unknown[]) {
  return {
    agent: 'a', generated_at: '2026-05-31T12:00:00Z', refresh_interval_secs: 5,
    summary: { cron_count: 0, active_cron_count: 0, failed_recent_cron_count: failedCount, today_cost_usd: 0 },
    crons: [], failed_runs: failedRuns,
    active: { foreground: [], background: [] },
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(ActivityView, props as any),
  })
  return renderToString(app)
}

describe('ActivityView failures card', () => {
  it('does not render the bad tone when there are no failures', async () => {
    const html = await render({ overview: activity(0, []), selectedRun: null, selectedRunId: null, loadingDetail: false, detailError: null })
    expect(html).not.toMatch(/metric-card[^"]*\bbad\b/)
  })
  it('renders the bad tone and section when failures exist', async () => {
    const failed = [{ id: 'run-1', kind: 'cron', producer_ref: 'job', status: 'failed', started_at: null, finished_at: '2026-05-31T11:00:00Z', exit_code: 1, delivery_required: false, delivery_status: 'none', delivery_kind: null, run_note: null, cost_usd: 0 }]
    const html = await render({ overview: activity(1, failed), selectedRun: null, selectedRunId: null, loadingDetail: false, detailError: null })
    expect(html).toMatch(/metric-card[^"]*\bbad\b/)
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/ActivityView.test.ts`
Expected: FAIL — current card is unconditionally `tone="bad"`, so the zero-case assertion fails.

- [ ] **Step 3: Wire the card + section**

In `crates/right-dashboard/frontend/src/views/ActivityView.vue` `<script setup>`, add:

```ts
import { computed, ref } from 'vue'
import CollapsibleSection from '../components/CollapsibleSection.vue'
import RunFailureList from '../components/RunFailureList.vue'
import { failureMetric } from '../components/failureMetric'
```

After the `defineProps<{...}>()` call, capture the props and add state:

```ts
const props = defineProps<{
  overview: OverviewResponse | null
  selectedRun: RunDetailResponse | null
  selectedRunId: string | null
  loadingDetail: boolean
  detailError: string | null
}>()

const failuresOpen = ref(false)
const failures = computed(() => failureMetric(props.overview?.summary.failed_recent_cron_count ?? 0))
```

> The component currently calls `defineProps<{...}>()` without binding it; assign it to `props`. Keep the existing `defineEmits` and `cronStatus` definitions.

Replace the Failures `MetricCard` line:

```vue
    <MetricCard label="Failures" :value="overview?.summary.failed_recent_cron_count ?? 0" tone="bad" />
```

with:

```vue
    <MetricCard
      label="Failures"
      :value="overview?.summary.failed_recent_cron_count ?? 0"
      :tone="failures.tone"
      :interactive="failures.interactive"
      @select="failuresOpen = !failuresOpen"
    />
```

After the closing `</section>` of the `<section class="metric-grid">` (the first section), add:

```vue
  <CollapsibleSection
    v-if="(overview?.summary.failed_recent_cron_count ?? 0) > 0"
    v-model:open="failuresOpen"
    title="Failures"
    :count="overview?.summary.failed_recent_cron_count ?? 0"
  >
    <RunFailureList :runs="overview?.failed_runs ?? []" />
  </CollapsibleSection>
```

- [ ] **Step 4: Run test + type-check**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/ActivityView.test.ts && devenv shell -- pnpm build`
Expected: tests PASS; type-check succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/ActivityView.vue crates/right-dashboard/frontend/src/views/ActivityView.test.ts
git commit -m "feat(dashboard): wire Activity failures card to failure list"
```

---

## Task 11: Wire `ReportsView`

Reports rows are learning events (no run detail) rendered inline, mirroring `LearningSignalPanel`.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`
- Test: `crates/right-dashboard/frontend/src/views/learning/ReportsView.test.ts` (new)

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/learning/ReportsView.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ReportsView from './ReportsView.vue'

function learning(failed: number, failedEvents: unknown[]) {
  return {
    agent: 'a', generated_at: '2026-05-31T12:00:00Z',
    flow_nodes: [], flow_edges: [], recent_learning_signals: [], warnings: [],
    lifecycle: {
      created_7d: 0, updated_7d: 0, failed_or_aborted_7d: failed,
      recent_successful_events: [], candidate_skill_names_7d: [],
      recent_failed_events: failedEvents,
    },
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(ReportsView, props as any),
  })
  return renderToString(app)
}

describe('ReportsView failed-skills card', () => {
  it('renders no bad tone at zero', async () => {
    const html = await render({ learning: learning(0, []) })
    expect(html).not.toMatch(/metric-card[^"]*\bbad\b/)
  })
  it('lists failed events when non-zero', async () => {
    const events = [{ skill_name: 'rightx', action: 'update', status: 'failed', message: 'boom', summary: null, created_at: '2026-05-31T11:00:00Z' }]
    const html = await render({ learning: learning(1, events) })
    expect(html).toMatch(/metric-card[^"]*\bbad\b/)
    expect(html).toContain('rightx')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/learning/ReportsView.test.ts`
Expected: FAIL — current "Failed 7d" card is unconditionally `tone="bad"`.

- [ ] **Step 3: Wire the card + inline failed-event list**

Replace the entire `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` with:

```vue
<script setup lang="ts">
import { computed, ref } from 'vue'

import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import CollapsibleSection from '../../components/CollapsibleSection.vue'
import MetricCard from '../../components/MetricCard.vue'
import StatusPill from '../../components/StatusPill.vue'
import { failureMetric } from '../../components/failureMetric'
import { shortDate } from '../../format'
import type { LearningOverviewResponse } from '../../types'

const props = defineProps<{
  learning: LearningOverviewResponse | null
}>()

const failuresOpen = ref(false)
const failures = computed(() => failureMetric(props.learning?.lifecycle.failed_or_aborted_7d ?? 0))
</script>

<template>
  <section v-if="learning?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in learning.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <section class="two-column wide-main">
    <LearningFlowChart
      :nodes="learning?.flow_nodes ?? []"
      :edges="learning?.flow_edges ?? []"
    />
    <LearningSignalPanel :signals="learning?.recent_learning_signals ?? []" />
  </section>

  <section class="metric-grid">
    <MetricCard label="Created 7d" :value="learning?.lifecycle.created_7d ?? 0" tone="ok" />
    <MetricCard label="Updated 7d" :value="learning?.lifecycle.updated_7d ?? 0" tone="active" />
    <MetricCard
      label="Failed 7d"
      :value="learning?.lifecycle.failed_or_aborted_7d ?? 0"
      :tone="failures.tone"
      :interactive="failures.interactive"
      @select="failuresOpen = !failuresOpen"
    />
  </section>

  <CollapsibleSection
    v-if="(learning?.lifecycle.failed_or_aborted_7d ?? 0) > 0"
    v-model:open="failuresOpen"
    title="Failed skills"
    :count="learning?.lifecycle.failed_or_aborted_7d ?? 0"
  >
    <div class="row-list">
      <div
        v-for="event in learning?.lifecycle.recent_failed_events ?? []"
        :key="`${event.skill_name}:${event.created_at}`"
        class="data-row static"
      >
        <span class="row-main">
          <strong>{{ event.skill_name }}</strong>
          <small>{{ event.action }} / {{ shortDate(event.created_at) }}</small>
          <small v-if="event.message" class="run-note-preview">{{ event.message }}</small>
        </span>
        <span class="row-side">
          <StatusPill :status="event.status" />
        </span>
      </div>
    </div>
  </CollapsibleSection>
</template>
```

- [ ] **Step 4: Run test + type-check**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test src/views/learning/ReportsView.test.ts && devenv shell -- pnpm build`
Expected: tests PASS; type-check succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/learning/ReportsView.vue crates/right-dashboard/frontend/src/views/learning/ReportsView.test.ts
git commit -m "feat(dashboard): wire Reports failed-skills card to failure list"
```

---

## Task 12: Final verification

- [ ] **Step 1: Full frontend test suite**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm test`
Expected: all suites PASS (incl. failureMetric, MetricCard, CollapsibleSection, RunFailureList, the three views, and pre-existing tests).

- [ ] **Step 2: Frontend production build (type-check + bundle)**

Run: `cd crates/right-dashboard/frontend && devenv shell -- pnpm build`
Expected: `vue-tsc --noEmit` clean, `vite build` succeeds.

- [ ] **Step 3: Mandatory full workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. (This also re-runs `build.rs`, which rebuilds the SPA.) Record any pre-existing unrelated failures.

- [ ] **Step 4: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 5: Manual smoke (interaction not covered by SSR tests)**

With the dashboard running, confirm in each tab: a `0` failures card is gray and not clickable; a non-zero card is red, clickable, and expands the full list; an Overview/Activity row expands to error+log via `runDetail`. Note results.

---

## Self-Review

**Spec coverage:**
- Zero → gray, non-zero → red, count→interactivity in one helper → Task 5 (`failureMetric`), applied in Tasks 9–11. ✓
- Untruncated lists, length == count, same window: Overview Task 2 (24h), Activity Task 3 (7d), Reports Task 4 (7d, explicitly no truncate). ✓
- Lists ship in existing payloads, no new endpoint: Tasks 2–4 add response fields only. ✓
- Shared `RunFailureList` for Overview+Activity, learning rows inline for Reports: Tasks 8, 9, 10, 11. ✓
- Activity count recomputed to failed cron runs in 7d (approved): Task 3. ✓
- `CollapsibleSection` controlled open so a card expands the list: Task 7, used in 9–11. ✓
- Dashboard-primitives rule (AsyncState/CollapsibleSection, decision logic in tested `.ts`): `failureMetric.ts`, `RunFailureList` uses `AsyncState`, sections use `CollapsibleSection`. ✓
- No prompt/ARCHITECTURE/migration changes. ✓

**Placeholder scan:** No TBD/TODO. Test seeding helpers are referenced by the existing-fixture names with an explicit instruction to reuse the module's real helpers rather than invent — this is honest about not duplicating fixtures unseen, not a placeholder for production code.

**Type consistency:** `failureMetric` returns `{ tone: 'default' | 'bad'; interactive: boolean }` and is consumed as `failures.tone` / `failures.interactive` in all three views. `MetricCard` props `interactive?: boolean` + emit `select` match the view bindings. `CollapsibleSection` `open?: boolean` + `update:open` match `v-model:open`. `RunFailureList` prop `runs: RunSummary[]` matches `recent_failed_runs` / `failed_runs`. Rust field names (`recent_failed_runs`, `failed_runs`, `recent_failed_events`) match their `types.ts` mirrors and the read-model literals. Shared SQL helpers (`RUN_SUMMARY_COLUMNS`, `RUN_SUMMARY_FROM`, `run_summary_from_row`) are defined once in Task 1 and imported in Tasks 2–3.

**Known SSR-test limitation (intentional):** `renderToString` cannot fire click events, so the `select` emit, the `v-model:open` toggle round-trip, and `RunFailureList`'s click→`runDetail` expansion are verified by Task 12 Step 5 (manual), not unit tests. Unit tests cover the rendered tone class, button-vs-article, controlled `open`, and row/empty rendering — the decision logic, not framework glue.
