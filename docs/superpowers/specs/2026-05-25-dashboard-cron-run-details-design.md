# Dashboard Cron Run Details

**Date:** 2026-05-25
**Status:** Design

## Problem

Cron runs already persist the data we need:

- `async_runs.run_note` is the cron run summary.
- `async_runs.delivery_json` stores the cron delivery decision, including
  `{"kind":"notify", ...}` and `{"kind":"silent", ...}`.
- `async_runs.delivery_required` and `async_runs.delivery_status` store whether
  delivery is queued and where it stands.

The dashboard already has a run detail endpoint and a right-side detail panel,
but the Activity mobile layout places that panel after the entire cron list.
When a user taps a run in a long list, the selected row changes in place while
the useful detail appears far below the current viewport. The interaction reads
as "nothing happened".

## Goal

Make cron run summaries and delivery decisions visible at the point of
selection, especially on Telegram Mini App mobile layouts.

Selecting a run should show the useful detail directly under that clicked run:
run note, notify/silent state, delivery status, delivery content/reason, and
errors when present.

## Non-goals

- Changing how cron jobs produce structured output.
- Changing delivery semantics or idle delivery rules.
- Adding cron run cancellation or retry controls.
- Reading full sandbox cron logs inline in every run row.

## Decisions

| Decision | Choice |
|---|---|
| Primary UX | Inline-expand the selected run directly beneath its row. |
| Summary source | Use `async_runs.run_note`. Do not reintroduce legacy `summary` naming. |
| Notify flag | Derive from parsed `delivery_json.kind`: `notify` vs `silent`; fall back to `delivery_required`/`delivery_status` if JSON is absent or malformed. |
| Detail endpoint | Keep `/api/v1/runs/{run_id}` and `/api/v1/activity/runs/{run_id}`. Use it for full selected-run detail. |
| List payload | Extend `RunSummary` only with bounded, display-safe fields needed for rows: `run_note`, `delivery_kind`, and `delivery_required`. |
| Mobile behavior | Inline detail is the canonical selected-run view. The side detail panel may remain for desktop, but mobile usability cannot depend on it. |

## Data Contract

`right-dashboard::api_types::RunSummary` gains optional summary/delivery fields:

```rust
// crates/right-dashboard/src/api_types.rs
pub struct RunSummary {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub delivery_status: String,
    pub delivery_required: bool,
    pub delivery_kind: Option<String>,
    pub run_note: Option<String>,
    pub cost_usd: Option<f64>,
}
```

The read model should parse `delivery_json` once while building run summaries.
Only `kind` is promoted into the list model. Full delivery JSON remains in
`RunDetailResponse.delivery`.

If `delivery_json` is malformed, list rendering should not fail. The summary
row should still render with `delivery_kind = null`, while the detail endpoint
continues to expose `delivery_error`.

## UI Behavior

Each recent run row shows:

- status, short run id, start time, and cost as today;
- a compact delivery badge:
  - `Notify` when `delivery_kind == "notify"`;
  - `Silent` when `delivery_kind == "silent"`;
  - otherwise the existing `delivery_status`;
- a one- or two-line run note preview when `run_note` exists.

When selected, the row expands immediately beneath itself with:

- full `run_note`;
- delivery kind/status;
- notify content or silent reason from `RunDetailResponse.delivery`;
- delivery parse error if present;
- cron error message if present.

The existing detail panel can still show the same selected run on wider
viewports. On narrow viewports it should not be required for the primary
workflow.

## Error Handling

If selecting a run fails, show the error inline under the selected row rather
than only in the side panel. The selected row remains visibly selected.

If the detail request is still loading, show a small inline loading state under
the row. Do not clear the row summary already present in the overview payload.

If polling refreshes the Activity overview and the selected run is no longer in
the recent-run window, clear the selection as today.

## Testing

Add narrow tests at the read-model/API layer:

- `RunSummary` includes `run_note`, `delivery_required`, and parsed
  `delivery_kind` for cron rows.
- Malformed `delivery_json` does not break activity overview.
- Activity API JSON exposes the new fields.

Add frontend coverage by type/build checks:

- `npm run build` in `crates/right-dashboard/frontend` catches TypeScript and
  template issues.

Implementation verification cadence:

- Start with a targeted failing Rust read-model test.
- After implementation, run targeted dashboard/read-model tests and the
  frontend build.
- Final verification for this worktree remains
  `devenv shell -- cargo test --workspace`.

## Architecture Docs

Because this touches dashboard activity read models and cron run presentation,
re-read and update if drifted:

- `docs/architecture/modules.md`
- `docs/architecture/sessions.md`

No `ARCHITECTURE.md` change is expected unless the dashboard API contract is
treated as a prescriptive rule there.
