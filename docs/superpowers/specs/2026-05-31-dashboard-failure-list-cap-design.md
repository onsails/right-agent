# Dashboard failure lists — bound the sample, keep the count truthful

Status: design (approved)
Date: 2026-05-31
Scope owner: andrey
Follow-up to: `2026-05-31-dashboard-failure-counts-design.md`

## Problem

The failure drill-down lists added in the failure-counts feature ship
**untruncated** inside the page payloads. The shared helper
`run_summary.rs::failed_runs_in_window` and the learning
`recent_failed_events` path emit every `status='failed'` row in their
window with no row cap.

The original review deferred this as "spec-compliant; revisit only as a
product decision." Two facts found while revisiting change the calculus:

1. **The window is the only bound.** Nothing prunes `async_runs` or
   `skill_learning_events` — there is no retention/DELETE path for either
   table (the only `retention_days` setting governs attachment-file
   cleanup). So the 24h/7d window predicate is the sole limit on payload
   size.
2. **The cost is recurring, not one-time.** `App.vue`'s poll timer
   (`REFRESH_INTERVAL_SECS = 5`) calls `refreshOverview()` **every 5s on
   every tab**, and `refreshActivity()` every 5s on the overview/activity
   tabs. The Overview payload always contains the full `recent_failed_runs`
   list, so a chronically-failing agent re-ships that list every 5s even
   while the user is on an unrelated tab and never expands the card.

Worst realistic case: a cron failing every ~5 min → ~288 failed runs in
the 24h Overview window → ~70 KB JSON re-sent every 5s (~50 MB/hr); a
1-min failing cron pushes this past ~250 MB/hr. Not a backend crash — the
symptoms are recurring mobile-bandwidth waste and DOM render-jank in the
webview, only for a badly-broken agent.

## Decision

For the case that motivates this — an agent that fails a lot — an
untruncated dump of every individual failure row is the *least* useful
shape. An operator triaging a broken agent needs two things: **how many**
(an honest total) and **what is failing** (a sample of the most recent, to
read the pattern). Nobody scrolls thousands of rows.

So: **bound each list to the newest N failures, and make the badge an
exact total.** Dropping "click to see literally every failure" is
deliberate; a truthful total plus a recent-N sample is better UX for the
pathological case and bounds the recurring payload.

## Why this is cheap (the key realization)

`failed_runs_in_window` **already materializes every precise-matched
`RunSummary` server-side today** — it builds them all into `out`, then
returns the whole vec. The server-side cost is already paid; the current
code merely also dumps the whole vec onto the wire.

The fix keeps that single scan. Precise window filtering still happens in
Rust over the full matched set (so the two-stage coarse-SQL +
precise-Rust filter is untouched and there is **no `LIMIT`-vs-precise-filter
pitfall**); we change only the tail: return the **exact count** of precise
matches *and* `out.into_iter().take(N)` for the list. No second query, no
SQL `LIMIT`, no new endpoint.

The same holds for the learning path: `learning_events_in_window` already
fetches and sorts the full matched set before its optional `truncate`.

## Contract change — no new API fields or endpoints

The existing per-surface count and list fields stay; only their semantics
shift:

| Surface | Count field (badge) → exact total M | List field → newest N |
|---|---|---|
| Overview | `recent_failures` | `recent_failed_runs` |
| Activity | `summary.failed_recent_cron_count` | `failed_runs` |
| Reports | `lifecycle.failed_or_aborted_7d` | `lifecycle.recent_failed_events` |

- The count field becomes the **exact windowed total M**, computed from
  the full precise-filtered set (no longer derived from `list.len()`).
- The list field becomes the **newest N** sample.
- When `M ≤ N`, the list length equals the count exactly — unchanged
  behavior for every realistic agent.
- The frontend derives "capped" itself from the two values it already
  receives: when `count > list.length`, the list is a sample.

This reverts the one-day-old `count == list.len()` derivation
(commit `9b53dd51`) back to an exact count — the trade we accept to get a
truthful badge plus a bounded list.

## Design

### N — the cap

`FAILURE_SAMPLE_LIMIT = 50`, one shared constant for all three surfaces.

- Any realistic agent (a handful of failures) still shows **all** of them;
  the cap engages only for genuinely-broken agents.
- 50 recent failures is an ample triage sample.
- Bounds the every-5s-poll payload to ~12 KB/surface instead of unbounded.
- (Precedent in these files is 30 — `SIGNAL_LIMIT`,
  `RECENT_LEARNING_SIGNAL_LIMIT`. 50 chosen because failures are the
  actionable signal; 30 would also be defensible.)

### Backend (`crates/right-dashboard/src/read_model`)

1. **`run_summary.rs::failed_runs_in_window`** → return
   `(i64, Vec<RunSummary>)`: the `i64` is the count of all precise matches
   (the loop already visits each); the vec is `out.into_iter().take(FAILURE_SAMPLE_LIMIT)`.
   Window filtering, ordering, and the coarse/precise two-stage logic are
   otherwise unchanged.

2. **`dashboard_overview.rs::recent_failed_runs` caller** — assign the
   returned count to `recent_failures` and the capped vec to
   `recent_failed_runs`. Remove `recent_failures = recent_failed_runs.len()`.

3. **`activity.rs::failed_cron_runs` caller** — assign the count to
   `summary.failed_recent_cron_count` and the capped vec to `failed_runs`.
   Remove `failed_recent_cron_count = failed_runs.len()`. The per-cron
   `cron_runs` (LIMIT 5) path is untouched.

4. **`learning.rs`** — the failed-events path returns both an exact total
   and a capped list from its single fetch+sort: set
   `failed_or_aborted_7d` to the full set length (before truncation) and
   `recent_failed_events` to the newest `FAILURE_SAMPLE_LIMIT`. The
   `recent_successful_events` path (truncated at `RECENT_EVENT_LIMIT`,
   no exact-total requirement) is unchanged.

### Frontend (`crates/right-dashboard/frontend/src`)

- **`components/RunFailureList.vue`** — when `count > runs.length`, render
  one subline above the rows: **"latest {runs.length} of {count}"**. The
  component already receives the runs; pass the surface's count alongside
  (a new `:total` prop, or compute the label in the parent view and pass
  it in — implementation choice for the plan).
- **`views/learning/ReportsView.vue`** — same subline above the inline
  failed-event rows, using `lifecycle.failed_or_aborted_7d` vs
  `recent_failed_events.length`.
- The badges already show the count; no badge change.
- Decision logic ("is this list capped, and what label") is a trivial
  derivation; if it needs a home, a one-line pure helper
  (`failureSampleLabel(total, shown)`) unit-tested per the
  dashboard-primitives rule. Loading/empty/error still go through
  `AsyncState`.

## Out of scope

- **No load-more / pagination.** We deliberately dropped "see every
  failure." If a real need to page beyond the newest N ever appears, that
  is a separate on-demand failures endpoint — explicitly not built now.
- **The `costs` LEFT JOIN full-scan.** `RUN_SUMMARY_FROM` re-aggregates
  the entire `usage_events` table on every run-summary query (hence every
  5s poll), growing with agent lifetime. A real scaling cost, but it
  affects all run-summary queries equally and is independent of this
  change. Its own follow-up, not fixed here.
- Any prompt, `ARCHITECTURE.md`, or DB-migration change. Read-only query
  shape changes over existing tables.

## Testing

Targeted first, full workspace last (per AGENTS.md cadence).

**Backend** (`cargo test -p right-dashboard`):
- Existing equality assertions use ≤13-row fixtures (`< 50`), so
  `list.len() == count` still holds and they stay green unchanged.
- Add one **over-cap** case per surface: seed `> FAILURE_SAMPLE_LIMIT`
  failures, assert `list.len() == FAILURE_SAMPLE_LIMIT` **and**
  `count == true_total` (`> FAILURE_SAMPLE_LIMIT`), and that the list is
  the newest N (ordering preserved).
- Rename/repurpose `recent_failed_events_includes_..._untruncated` to
  reflect the 50-cap (it currently asserts "not truncated at 10"; the new
  truth is "not truncated below 50, capped at 50").

**Frontend** (vitest / Vue SSR `renderToString`):
- If a `failureSampleLabel(total, shown)` helper is extracted:
  `total <= shown` → no label; `total > shown` → "latest {shown} of {total}".
- `RunFailureList`: with `count > runs.length`, the subline renders;
  with `count == runs.length`, it does not (regression).
- `ReportsView`: capped failed-event list renders the subline.

**Final (mandatory):** `devenv shell -- cargo test --workspace` and the
frontend test/build (`build.rs` bundles the SPA).

## Files touched

- `crates/right-dashboard/src/read_model/run_summary.rs` — return
  `(count, capped list)`; add `FAILURE_SAMPLE_LIMIT`.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — exact
  count + capped list; drop count-from-len.
- `crates/right-dashboard/src/read_model/activity.rs` — exact count +
  capped list; drop count-from-len.
- `crates/right-dashboard/src/read_model/learning.rs` — exact total +
  capped failed-events list.
- `crates/right-dashboard/frontend/src/components/RunFailureList.vue` —
  "latest N of M" subline.
- `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` —
  same subline for failed events.
- (optional) `crates/right-dashboard/frontend/src/components/failureSampleLabel.ts`
  (+ `.test.ts`) — pure label helper if extracted.
- Backend + frontend tests as above.
