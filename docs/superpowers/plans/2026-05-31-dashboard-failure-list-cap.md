# Dashboard Failure-List Cap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound each dashboard failure drill-down list to the newest 50 rows while keeping its badge an exact windowed total, so a chronically-failing agent cannot bloat the every-5s poll payload.

**Architecture:** The shared read-model scans already materialize every precise-matched failure row server-side; we change only their tail to return `(exact_count, newest_N_rows)` instead of the whole vec. No new API fields or endpoints — the existing count fields become exact totals (may exceed the list length), the list fields become capped samples, and the frontend derives a "latest N of M" subline from the two values it already receives.

**Tech Stack:** Rust (edition 2024, `right-dashboard` crate, `turso`/`right-db`), Vue 3 `<script setup>` + TypeScript, vitest SSR (`@vue/server-renderer`), pnpm. All commands run through `devenv shell --`.

**Spec:** `docs/superpowers/specs/2026-05-31-dashboard-failure-list-cap-design.md`

---

## File map

| File | Change |
|---|---|
| `crates/right-dashboard/src/read_model.rs` | Add `pub(crate) const FAILURE_SAMPLE_LIMIT: usize = 50`. |
| `crates/right-dashboard/src/read_model/run_summary.rs` | `failed_runs_in_window` returns `(usize, Vec<RunSummary>)` — exact count + capped list. |
| `crates/right-dashboard/src/read_model/dashboard_overview.rs` | Caller destructures count+list; drop count-from-len; over-cap test. |
| `crates/right-dashboard/src/read_model/activity.rs` | Caller destructures count+list; drop count-from-len; over-cap test. |
| `crates/right-dashboard/src/read_model/learning.rs` | `learning_events_in_window` returns `(usize, Vec)`; failed path caps + exact total; rename old test; over-cap test. |
| `crates/right-dashboard/frontend/src/components/failureSampleLabel.ts` (+ `.test.ts`) | New pure helper. |
| `crates/right-dashboard/frontend/src/components/RunFailureList.vue` (+ `.test.ts`) | `total` prop + sample subline. |
| `crates/right-dashboard/frontend/src/views/OverviewView.vue` | Pass `:total`. |
| `crates/right-dashboard/frontend/src/views/ActivityView.vue` | Pass `:total`. |
| `crates/right-dashboard/frontend/src/components/FailedSkillList.vue` (+ `.test.ts`) | New dumb component: Reports failed-event rows + sample subline (SSR-testable). |
| `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` | Use `FailedSkillList` inside the collapsible; drop inline rows + now-unused imports. |

**No `api_types.rs` / `types.ts` change** — `recent_failures: i64`, `failed_recent_cron_count: usize`, `failed_or_aborted_7d: i64` and the three list fields already exist; only their runtime semantics change.

---

## Task 1: Backend — run-summary surfaces (Overview + Activity)

The `failed_runs_in_window` helper is shared by both run-summary surfaces; its signature change touches both callers, so they ship in one commit.

**Files:**
- Modify: `crates/right-dashboard/src/read_model.rs` (after the `ReadModelError` enum, ~line 35)
- Modify: `crates/right-dashboard/src/read_model/run_summary.rs:52-85`
- Modify: `crates/right-dashboard/src/read_model/dashboard_overview.rs:34-38` and `:848-855`
- Modify: `crates/right-dashboard/src/read_model/activity.rs:77-78` and `:282-289`
- Test: `crates/right-dashboard/src/read_model/dashboard_overview.rs` (tests module) and `crates/right-dashboard/src/read_model/activity.rs` (tests module)

- [ ] **Step 1: Write the two failing over-cap tests**

Append to the `mod tests` in `crates/right-dashboard/src/read_model/dashboard_overview.rs` (before the closing `}` of the module):

```rust
    #[tokio::test]
    async fn recent_failed_runs_caps_at_sample_limit_with_true_count() {
        let (_dir, conn) = fixture().await;
        // 51 failed runs in the 24h window (> FAILURE_SAMPLE_LIMIT = 50).
        for i in 0..51 {
            conn.execute(
                "INSERT INTO async_runs (
                    id, kind, producer_ref, run_session_id, target_chat_id,
                    status, finished_at, exit_code, delivery_required, delivery_status,
                    created_at, updated_at
                 ) VALUES (?1, 'cron', 'daily', ?2, 123,
                    'failed', '2026-05-31T11:00:00Z', 1, 1, 'pending',
                    '2026-05-31T11:00:00Z', '2026-05-31T11:00:00Z')",
                right_db::params![format!("run-{i:03}"), format!("session-{i:03}")],
            )
            .await
            .unwrap();
        }

        let response = dashboard_overview(
            &conn,
            DashboardOverviewInput {
                agent: "alpha".to_string(),
                generated_at: "2026-05-31T12:00:00Z".to_string(),
                foreground_active_count: 0,
                sandbox: crate::api_types::OverviewSandboxStatus {
                    state: "configured".to_string(),
                    detail: None,
                },
            },
        )
        .await
        .unwrap();

        assert_eq!(response.recent_failures, 51);
        assert_eq!(response.recent_failed_runs.len(), 50);
    }
```

Append to the `mod tests` in `crates/right-dashboard/src/read_model/activity.rs` (before the closing `}`):

```rust
    #[tokio::test]
    async fn failed_cron_runs_caps_at_sample_limit_with_true_count() {
        let (_dir, conn) = fixture().await;
        // 51 failed cron runs in the 7d window (> FAILURE_SAMPLE_LIMIT = 50).
        for i in 0..51 {
            conn.execute(
                "INSERT INTO async_runs (
                    id, kind, producer_ref, run_session_id, target_chat_id,
                    status, finished_at, delivery_required, delivery_status,
                    created_at, updated_at
                 ) VALUES (?1, 'cron', 'job-a', ?2, 123,
                    'failed', '2026-05-31T11:00:00Z', 0, 'none',
                    '2026-05-31T11:00:00Z', '2026-05-31T11:00:00Z')",
                right_db::params![format!("run-{i:03}"), format!("session-{i:03}")],
            )
            .await
            .unwrap();
        }

        let response = activity_overview(
            &conn,
            ActivityOverviewInput {
                agent: "agent-a".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 30,
                foreground: vec![],
            },
        )
        .await
        .unwrap();

        assert_eq!(response.summary.failed_recent_cron_count, 51);
        assert_eq!(response.failed_runs.len(), 50);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devenv shell -- cargo test -p right-dashboard caps_at_sample_limit_with_true_count`
Expected: both FAIL on the `.len()` assertion — the lists are currently untruncated (51), so `assert_eq!(..., 50)` fails. (`recent_failures`/`failed_recent_cron_count` are still 51 because they are derived from the list length today.)

- [ ] **Step 3: Add the `FAILURE_SAMPLE_LIMIT` constant**

In `crates/right-dashboard/src/read_model.rs`, insert immediately after the `ReadModelError` enum closing `}` (currently line 35), before `pub type OverviewInput`:

```rust
/// Upper bound on how many failure rows each dashboard surface ships in a
/// single payload. The badge count stays the exact windowed total; this caps
/// only the inline sample list (newest-first) so a chronically-failing agent
/// cannot bloat the every-5s poll payload. See
/// docs/superpowers/specs/2026-05-31-dashboard-failure-list-cap-design.md.
pub(crate) const FAILURE_SAMPLE_LIMIT: usize = 50;
```

- [ ] **Step 4: Change `failed_runs_in_window` to return count + capped list**

In `crates/right-dashboard/src/read_model/run_summary.rs`, update the signature (line 52-57) return type and the doc comment (lines 47-51), then the tail (lines 76-85).

Replace the doc comment + signature:

```rust
/// Failed `async_runs` whose window timestamp falls in `[since, now]`, newest
/// first. `kind` optionally restricts to a single run kind (e.g. `"cron"`);
/// `None` matches all kinds. Returns the exact count of matching rows and the
/// newest `FAILURE_SAMPLE_LIMIT` of them — the count may exceed the list
/// length. Shared by the dashboard overview (24h, all kinds) and activity
/// overview (7d, cron-only) so both stay aligned with `RUN_SUMMARY_COLUMNS`.
pub(super) async fn failed_runs_in_window(
    conn: &Connection,
    now: &DateTime<Utc>,
    since: &DateTime<Utc>,
    kind: Option<&str>,
) -> Result<(usize, Vec<RunSummary>), ReadModelError> {
```

Replace the tail (the `let mut out = Vec::new();` loop through `Ok(out)`):

```rust
    let mut out = Vec::new();
    for row in rows {
        let (run, win_ts) = row?;
        let ts = parse_utc(&win_ts)?;
        if ts >= *since && ts <= *now {
            out.push(run);
        }
    }
    let total = out.len();
    out.truncate(super::FAILURE_SAMPLE_LIMIT);
    Ok((total, out))
}
```

- [ ] **Step 5: Update the Overview caller**

In `crates/right-dashboard/src/read_model/dashboard_overview.rs`, replace lines 34-38 (the `let recent_failed_runs = ...` through `let recent_failures = recent_failed_runs.len() as i64;`):

```rust
    // The list is capped at FAILURE_SAMPLE_LIMIT; the count is the exact 24h
    // windowed total and may exceed the list length.
    let (recent_failures_count, recent_failed_runs) =
        recent_failed_runs(conn, &input.generated_at).await?;
    let recent_failures = recent_failures_count as i64;
```

Replace the `recent_failed_runs` helper (lines 848-855):

```rust
async fn recent_failed_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<(usize, Vec<RunSummary>), ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::hours(24);
    super::run_summary::failed_runs_in_window(conn, &now, &since, None).await
}
```

- [ ] **Step 6: Update the Activity caller**

In `crates/right-dashboard/src/read_model/activity.rs`, replace lines 77-78 (`let failed_runs = ...` and `let failed_recent_cron_count = failed_runs.len();`):

```rust
    let (failed_recent_cron_count, failed_runs) =
        failed_cron_runs(conn, &input.generated_at).await?;
```

Replace the `failed_cron_runs` helper (lines 282-289):

```rust
async fn failed_cron_runs(
    conn: &Connection,
    generated_at: &str,
) -> Result<(usize, Vec<RunSummary>), ReadModelError> {
    let now = parse_utc(generated_at)?;
    let since = now - Duration::days(7);
    super::run_summary::failed_runs_in_window(conn, &now, &since, Some("cron")).await
}
```

- [ ] **Step 7: Run the dashboard package tests to verify green**

Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS — the two new over-cap tests pass (count 51, list 50); the existing `recent_failed_runs_matches_failure_count_and_lists_each_run` and `failed_runs_lists_failed_cron_runs_in_window_and_matches_count` (2-row fixtures, ≤ 50) still pass because list length equals count when count ≤ 50.

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/src/read_model.rs \
        crates/right-dashboard/src/read_model/run_summary.rs \
        crates/right-dashboard/src/read_model/dashboard_overview.rs \
        crates/right-dashboard/src/read_model/activity.rs
git commit -m "feat(dashboard): cap run-failure lists, keep exact badge count"
```

---

## Task 2: Backend — learning surface

`learning_events_in_window` is shared by the successful (truncated at 10) and failed paths; its signature change touches both, so they ship together.

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning.rs:749-766` (`learning_lifecycle`), `:772-832` (`learning_events_in_window`), `:834-849` (`recent_successful_events`), `:851-860` (`recent_failed_events`)
- Test: `crates/right-dashboard/src/read_model/learning.rs` (tests module, `:1083-1153`)

- [ ] **Step 1: Write the failing over-cap test**

Append to the `mod tests` in `crates/right-dashboard/src/read_model/learning.rs` (before the closing `}`):

```rust
    #[tokio::test]
    async fn recent_failed_events_caps_at_sample_limit_with_true_count() {
        let (_dir, conn) = fixture().await;
        // 51 failed finish events in the 7d window (> FAILURE_SAMPLE_LIMIT = 50).
        for i in 0..51 {
            conn.execute(
                "INSERT INTO skill_learning_events (
                    invocation_id, agent_name, action, skill_name, phase, status,
                    message, summary, event_refs_json, hint_outcome, created_at
                 ) VALUES (?1, 'agent', 'create', ?2, 'finish', 'failed',
                    'fail msg', 'fail summary', '[]', NULL, '2026-05-31T10:00:00Z')",
                right_db::params![format!("inv-{i:03}"), format!("rightx-skill-{i:03}")],
            )
            .await
            .unwrap();
        }

        let response = learning_overview(
            &conn,
            LearningOverviewInput {
                agent: "agent".to_owned(),
                generated_at: "2026-05-31T12:00:00Z".to_owned(),
                refresh_interval_secs: 5,
            },
        )
        .await
        .unwrap();

        assert_eq!(response.lifecycle.failed_or_aborted_7d, 51);
        assert_eq!(response.lifecycle.recent_failed_events.len(), 50);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devenv shell -- cargo test -p right-dashboard recent_failed_events_caps_at_sample_limit_with_true_count`
Expected: FAIL on `.len()` — `recent_failed_events` is currently untruncated (51), so `assert_eq!(..., 50)` fails.

- [ ] **Step 3: Change `learning_events_in_window` to return count + (optionally capped) list**

In `crates/right-dashboard/src/read_model/learning.rs`, update the signature (line 772-779) return type:

```rust
async fn learning_events_in_window(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
    statuses: [&str; 2],
    limit: Option<usize>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
```

Replace the tail (lines 827-831, `events.sort_by(...)` through `}`):

```rust
    events.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let total = events.len();
    if let Some(limit) = limit {
        events.truncate(limit);
    }
    Ok((
        total,
        events.into_iter().map(|(_, _, event)| event).collect(),
    ))
}
```

- [ ] **Step 4: Update `recent_successful_events` (discards the count)**

Replace lines 834-849:

```rust
async fn recent_successful_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let (_total, events) = learning_events_in_window(
        conn,
        agent,
        since_7d,
        now,
        ["created", "updated"],
        Some(RECENT_EVENT_LIMIT as usize),
    )
    .await?;
    Ok(events)
}
```

- [ ] **Step 5: Update `recent_failed_events` (returns exact total + capped list)**

Replace lines 851-860:

```rust
async fn recent_failed_events(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<(usize, Vec<LearningEventSummary>), ReadModelError> {
    // Capped at FAILURE_SAMPLE_LIMIT (newest-first); the returned total is the
    // exact windowed count, which may exceed the list length and feeds
    // `failed_or_aborted_7d`.
    learning_events_in_window(
        conn,
        agent,
        since_7d,
        now,
        ["failed", "aborted"],
        Some(super::FAILURE_SAMPLE_LIMIT),
    )
    .await
}
```

- [ ] **Step 6: Update `learning_lifecycle` to take the exact total from the failed scan**

Replace lines 749-766:

```rust
async fn learning_lifecycle(
    conn: &Connection,
    agent: &str,
    since_7d: &DateTime<Utc>,
    now: &DateTime<Utc>,
) -> Result<LearningLifecycle, ReadModelError> {
    let (failed_or_aborted_7d, recent_failed_events) =
        recent_failed_events(conn, agent, since_7d, now).await?;
    Ok(LearningLifecycle {
        created_7d: writer_status_count_in_window(conn, agent, "created", since_7d, now).await?,
        updated_7d: writer_status_count_in_window(conn, agent, "updated", since_7d, now).await?,
        // Exact windowed total from the failed-events scan; the list it returns
        // is capped at FAILURE_SAMPLE_LIMIT.
        failed_or_aborted_7d: failed_or_aborted_7d as i64,
        recent_successful_events: recent_successful_events(conn, agent, since_7d, now).await?,
        recent_failed_events,
        candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d, now).await?,
    })
}
```

- [ ] **Step 7: Rename the existing ≤-cap test to drop the stale "untruncated" framing**

In `crates/right-dashboard/src/read_model/learning.rs` tests, rename `recent_failed_events_includes_failed_and_aborted_untruncated` (line 1084) to `recent_failed_events_includes_failed_and_aborted`, and update its two leading comment lines (1087-1088) to:

```rust
        // Seed 12 "failed" + 1 "aborted" finish events (13 total, below the
        // FAILURE_SAMPLE_LIMIT of 50, so the full set is returned).
```

Update the inner assertion message (currently `"expected >= 13 (not truncated at 10), got {}"`) to:

```rust
            "expected >= 13 (full set below the 50 cap), got {}",
```

(The assertions themselves — `len == failed_or_aborted_7d`, `len >= 13`, includes aborted, all failed/aborted — stay valid because 13 ≤ 50.)

- [ ] **Step 8: Run the dashboard package tests to verify green**

Run: `devenv shell -- cargo test -p right-dashboard`
Expected: PASS — the new over-cap test passes (51 total, 50 listed); the renamed 13-row test passes; all other learning tests unchanged.

- [ ] **Step 9: Commit**

```bash
git add crates/right-dashboard/src/read_model/learning.rs
git commit -m "feat(dashboard): cap learning failed-events list, keep exact total"
```

---

## Task 3: Frontend — `failureSampleLabel` pure helper

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/failureSampleLabel.ts`
- Test: `crates/right-dashboard/frontend/src/components/failureSampleLabel.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/failureSampleLabel.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { failureSampleLabel } from './failureSampleLabel'

describe('failureSampleLabel', () => {
  it('returns null when the full set is shown', () => {
    expect(failureSampleLabel(3, 3)).toBeNull()
    expect(failureSampleLabel(2, 5)).toBeNull()
    expect(failureSampleLabel(0, 0)).toBeNull()
  })
  it('labels the sample when the total exceeds the shown rows', () => {
    expect(failureSampleLabel(137, 50)).toBe('latest 50 of 137')
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run (from `crates/right-dashboard/frontend/`):
`(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/failureSampleLabel.test.ts)`
Expected: FAIL — `Cannot find module './failureSampleLabel'`.

- [ ] **Step 3: Implement the helper**

Create `crates/right-dashboard/frontend/src/components/failureSampleLabel.ts`:

```ts
/**
 * Label for a capped failure sample, or `null` when every failure is shown.
 * The badge already carries the exact total; this only annotates the list when
 * it is a newest-first sample (`total > shown`).
 */
export function failureSampleLabel(total: number, shown: number): string | null {
  if (total > shown) {
    return `latest ${shown} of ${total}`
  }
  return null
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/failureSampleLabel.test.ts)`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/failureSampleLabel.ts \
        crates/right-dashboard/frontend/src/components/failureSampleLabel.test.ts
git commit -m "feat(dashboard): add failureSampleLabel helper"
```

---

## Task 4: Frontend — `RunFailureList` sample subline + view wiring

Adding a required `total` prop breaks `OverviewView`/`ActivityView` typecheck until they pass it, so the component change and both callers ship in one commit.

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/RunFailureList.vue`
- Modify: `crates/right-dashboard/frontend/src/components/RunFailureList.test.ts`
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue:89`
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.vue:54`

- [ ] **Step 1: Update the component test (existing renders get `total`; add subline cases)**

In `crates/right-dashboard/frontend/src/components/RunFailureList.test.ts`, replace the whole `describe('RunFailureList', ...)` block:

```ts
describe('RunFailureList', () => {
  it('renders one row per failed run', async () => {
    const html = await render({ runs: [failedRun('run-aaaaaaaa'), failedRun('run-bbbbbbbb')], total: 2 })
    expect(html).toContain('cron')
    expect(html).toContain('job-x')
  })
  it('shows an empty hint when there are no runs', async () => {
    const html = await render({ runs: [], total: 0 })
    expect(html).toContain('No failures')
  })
  it('shows a sample label when the total exceeds the shown rows', async () => {
    const runs = Array.from({ length: 50 }, (_, i) => failedRun(`run-${i.toString().padStart(8, '0')}`))
    const html = await render({ runs, total: 137 })
    expect(html).toContain('latest 50 of 137')
  })
  it('omits the sample label when all failures are shown', async () => {
    const html = await render({ runs: [failedRun('run-aaaaaaaa')], total: 1 })
    expect(html).not.toContain('latest')
  })
})
```

- [ ] **Step 2: Run the component test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/RunFailureList.test.ts)`
Expected: FAIL — "shows a sample label…" finds no `latest 50 of 137` (component has no subline yet).

- [ ] **Step 3: Add the `total` prop + sample subline to the component**

In `crates/right-dashboard/frontend/src/components/RunFailureList.vue`, replace the script-setup head (lines 1-16, through the `defineProps` block):

```vue
<script setup lang="ts">
import { computed, ref } from 'vue'

import { runDetail } from '../api'
import { money, shortDate, shortId, statusTone } from '../format'
import type { RunDetailResponse, RunSummary } from '../types'
import AsyncState from './AsyncState.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  runs: RunSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.runs.length))

const selectedId = ref<string | null>(null)
const detail = ref<RunDetailResponse | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
```

Then wrap the list branch of the template. Replace the empty-state line + list opening (lines 47-48):

```vue
  <p v-if="runs.length === 0" class="muted-line">No failures</p>
  <template v-else>
    <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
    <div class="row-list">
```

…and replace the matching closing `</div>` of the row-list (line 107) with:

```vue
    </div>
  </template>
```

(The `<template v-for="run in runs">…</template>` rows in between are unchanged.)

- [ ] **Step 4: Wire `:total` in both views**

In `crates/right-dashboard/frontend/src/views/OverviewView.vue`, replace line 89:

```vue
      <RunFailureList :runs="overview?.recent_failed_runs ?? []" :total="overview?.recent_failures ?? 0" />
```

In `crates/right-dashboard/frontend/src/views/ActivityView.vue`, replace line 54:

```vue
    <RunFailureList :runs="overview?.failed_runs ?? []" :total="overview?.summary.failed_recent_cron_count ?? 0" />
```

- [ ] **Step 5: Run component test + typecheck + affected view tests to verify green**

Run:
```
(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/RunFailureList.test.ts src/views/OverviewView.test.ts src/views/ActivityView.test.ts && devenv shell -- pnpm typecheck)
```
Expected: PASS — all four RunFailureList cases pass; the Overview/Activity view tests still pass (badge counts unchanged); `vue-tsc` reports no errors (the required `total` prop is now supplied by both views).

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/RunFailureList.vue \
        crates/right-dashboard/frontend/src/components/RunFailureList.test.ts \
        crates/right-dashboard/frontend/src/views/OverviewView.vue \
        crates/right-dashboard/frontend/src/views/ActivityView.vue
git commit -m "feat(dashboard): show 'latest N of M' on run-failure samples"
```

---

## Task 5: Frontend — extract `FailedSkillList` component with sample subline

`ReportsView` renders its failed-event rows inline inside `CollapsibleSection`, whose body is `v-if="isOpen"` and starts collapsed — so those rows (and any subline placed there) never appear in an SSR render, and the view owns `failuresOpen` internally with no way for a test to open it. Extracting the list into a dumb `FailedSkillList` component (analogous to `RunFailureList`, but for `LearningEventSummary` with no run detail) makes the rows and subline directly SSR-testable and keeps the view focused. This honors the spec's Decision 6 (LearningEventSummary rows, no run-detail) and closes the gap where Reports rows had no row-level test.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/FailedSkillList.vue`
- Test: `crates/right-dashboard/frontend/src/components/FailedSkillList.test.ts`
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`

- [ ] **Step 1: Write the failing component test**

Create `crates/right-dashboard/frontend/src/components/FailedSkillList.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import FailedSkillList from './FailedSkillList.vue'

function failedEvent(skill: string) {
  return {
    skill_name: skill,
    action: 'create',
    status: 'failed',
    message: 'boom',
    summary: null,
    created_at: '2026-05-31T11:00:00Z',
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(FailedSkillList, props as any),
  })
  return renderToString(app)
}

describe('FailedSkillList', () => {
  it('renders one row per failed event', async () => {
    const html = await render({ events: [failedEvent('rightx-a'), failedEvent('rightx-b')], total: 2 })
    expect(html).toContain('rightx-a')
    expect(html).toContain('rightx-b')
  })
  it('shows an empty hint when there are no events', async () => {
    const html = await render({ events: [], total: 0 })
    expect(html).toContain('No failures')
  })
  it('shows a sample label when the total exceeds the shown rows', async () => {
    const events = Array.from({ length: 50 }, (_, i) => failedEvent(`rightx-${i}`))
    const html = await render({ events, total: 137 })
    expect(html).toContain('latest 50 of 137')
  })
  it('omits the sample label when all events are shown', async () => {
    const html = await render({ events: [failedEvent('rightx-a')], total: 1 })
    expect(html).not.toContain('latest')
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/FailedSkillList.test.ts)`
Expected: FAIL — `Cannot find module './FailedSkillList.vue'`.

- [ ] **Step 3: Create the component**

Create `crates/right-dashboard/frontend/src/components/FailedSkillList.vue` (the row markup is moved verbatim from `ReportsView.vue:55-70`):

```vue
<script setup lang="ts">
import { computed } from 'vue'

import { shortDate } from '../format'
import type { LearningEventSummary } from '../types'
import StatusPill from './StatusPill.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  events: LearningEventSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.events.length))
</script>

<template>
  <p v-if="events.length === 0" class="muted-line">No failures</p>
  <template v-else>
    <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
    <div class="row-list">
      <div
        v-for="event in events"
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
  </template>
</template>
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/components/FailedSkillList.test.ts)`
Expected: PASS — all four cases.

- [ ] **Step 5: Wire the component into ReportsView and drop the now-unused inline markup**

In `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`, replace the import block (lines 4-11) — add `FailedSkillList`, remove the now-unused `StatusPill` and `shortDate` imports:

```ts
import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import CollapsibleSection from '../../components/CollapsibleSection.vue'
import FailedSkillList from '../../components/FailedSkillList.vue'
import MetricCard from '../../components/MetricCard.vue'
import { failureMetric } from '../../components/failureMetric'
import type { LearningOverviewResponse } from '../../types'
```

Then replace the `CollapsibleSection` body — the entire `<div class="row-list">…</div>` block (lines 55-70) — with the component:

```vue
    <FailedSkillList
      :events="learning?.lifecycle.recent_failed_events ?? []"
      :total="learning?.lifecycle.failed_or_aborted_7d ?? 0"
    />
```

- [ ] **Step 6: Run ReportsView test + typecheck to verify green**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm exec vitest run src/views/learning/ReportsView.test.ts && devenv shell -- pnpm typecheck)`
Expected: PASS — the existing ReportsView tests (header/card assertions) still pass; `vue-tsc` reports no errors and no unused-import complaints.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/FailedSkillList.vue \
        crates/right-dashboard/frontend/src/components/FailedSkillList.test.ts \
        crates/right-dashboard/frontend/src/views/learning/ReportsView.vue
git commit -m "feat(dashboard): extract FailedSkillList with 'latest N of M' sample"
```

---

## Task 6: Final verification

**No code changes** — run the full suites the project mandates and confirm the SPA bundles.

- [ ] **Step 1: Full frontend test + typecheck**

Run: `(cd crates/right-dashboard/frontend && devenv shell -- pnpm test && devenv shell -- pnpm typecheck)`
Expected: all vitest suites PASS; `vue-tsc --noEmit` reports no errors.

- [ ] **Step 2: SPA build (exercises `build.rs` → vite build)**

Run: `devenv shell -- cargo build -p right-dashboard`
Expected: builds clean; `build.rs` runs `pnpm install` + `vite build` and finds the bundled output.

- [ ] **Step 3: Mandatory full workspace test**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS (record any pre-existing unrelated failures; nothing in this change should regress).

- [ ] **Step 4: Confirm clean tree**

Run: `git status --short`
Expected: empty — every task committed.

---

## Self-review notes

- **Spec coverage:** count→exact total (Tasks 1–2), list→newest 50 (Tasks 1–2), `FAILURE_SAMPLE_LIMIT = 50` (Task 1 Step 3), no new API fields (file map note), "latest N of M" subline on all three surfaces (Overview/Activity via `RunFailureList` in Task 4; Reports via `FailedSkillList` in Task 5), pure helper extracted + unit-tested per the dashboard-primitives rule (Task 3), existing ≤-cap tests stay green + over-cap test per surface + learning test rename (Tasks 1–2). Out-of-scope items (pagination, `costs` JOIN) are untouched by design.
- **Refinement beyond the spec's wording:** the spec said Reports rows render "inline"; the plan extracts them into a dumb `FailedSkillList` component (Task 5) because `CollapsibleSection`'s body is `v-if="isOpen"` and starts collapsed, so an inline subline can't be SSR-tested. This keeps Decision 6 intent (LearningEventSummary rows, no run detail) and makes the rows + subline testable — a structural improvement, not a behavior change.
- **Types:** `failed_runs_in_window` and `learning_events_in_window` both return `(usize, Vec<…>)`; Overview casts `usize as i64` for `recent_failures`, Activity assigns `usize` directly to `failed_recent_cron_count`, learning casts `usize as i64` for `failed_or_aborted_7d` — matching the existing field types (`i64`, `usize`, `i64`). `FAILURE_SAMPLE_LIMIT: usize` matches `Vec::truncate`. Helper name `failureSampleLabel(total, shown)` is consistent across Tasks 3/4/5.
- **No placeholders:** every code step shows complete code; every run step gives an exact command and expected result.
