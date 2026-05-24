# Dashboard Cron Run Details Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show cron run summaries and notify/silent delivery state directly at the selected run row in the dashboard Activity view.

**Architecture:** Promote bounded cron run detail fields from `async_runs` into the existing dashboard `RunSummary` read model, while keeping full delivery/log detail behind the existing run detail endpoint. The Vue Activity view renders a compact row preview plus an inline selected-run detail block, so mobile users do not need to scroll past the whole cron list to see what changed.

**Tech Stack:** Rust 2024, `right-dashboard` read models over SQLite via `rusqlite`, Axum dashboard routes in `right-bot`, Vue 3 + TypeScript + Vite frontend.

---

## File Structure

- Modify `crates/right-dashboard/src/api_types.rs`: extend `RunSummary` with `delivery_required`, `delivery_kind`, and `run_note`.
- Modify `crates/right-dashboard/src/read_model/activity.rs`: select/parse the new fields from `async_runs`, add tests for summary fields and malformed delivery JSON.
- Modify `crates/bot/src/telegram/dashboard.rs`: strengthen Activity API JSON test coverage for the new fields.
- Modify `crates/right-dashboard/frontend/src/types.ts`: mirror the Rust DTO fields.
- Modify `crates/right-dashboard/frontend/src/format.ts`: add small delivery display helpers.
- Modify `crates/right-dashboard/frontend/src/views/ActivityView.vue`: render run note preview, delivery badge, and inline selected-run detail.
- Modify `crates/right-dashboard/frontend/src/App.vue`: add CSS for the new inline detail and row preview.
- Rebuild `crates/right-dashboard/static/dashboard/`: generated Vite assets checked into the repo.
- Review `docs/architecture/modules.md` and `docs/architecture/sessions.md`; update only if implementation changes documented behavior beyond the current descriptions.

## Task 1: Extend RunSummary Read Model

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`
- Modify: `crates/right-dashboard/src/read_model/activity.rs`

- [ ] **Step 1: Write failing read-model tests**

Add these tests inside `mod tests` in `crates/right-dashboard/src/read_model/activity.rs`:

```rust
#[test]
fn activity_overview_includes_cron_run_note_and_delivery_fields() {
    let (_dir, conn) = fixture();
    conn.execute(
        "UPDATE async_runs
         SET run_note = ?1,
             delivery_json = ?2,
             delivery_required = 0,
             delivery_status = 'none'
         WHERE id = 'run-1'",
        (
            "Checked Composio status; still degraded.",
            r#"{"kind":"silent","reason":"still degraded"}"#,
        ),
    )
    .unwrap();

    let response = activity_overview(
        &conn,
        ActivityOverviewInput {
            agent: "agent-a".to_owned(),
            generated_at: "2026-05-20T10:00:00Z".to_owned(),
            refresh_interval_secs: 30,
            foreground: vec![],
        },
    )
    .unwrap();

    let run = &response.crons[0].recent_runs[0];
    assert_eq!(
        run.run_note.as_deref(),
        Some("Checked Composio status; still degraded.")
    );
    assert!(!run.delivery_required);
    assert_eq!(run.delivery_status, "none");
    assert_eq!(run.delivery_kind.as_deref(), Some("silent"));
}

#[test]
fn activity_overview_ignores_malformed_delivery_json_in_run_summary() {
    let (_dir, conn) = fixture();
    conn.execute(
        "UPDATE async_runs
         SET run_note = ?1,
             delivery_json = ?2,
             delivery_required = 1,
             delivery_status = 'pending'
         WHERE id = 'run-1'",
        ("Malformed delivery should not break overview.", r#"{bad-json"#),
    )
    .unwrap();

    let response = activity_overview(
        &conn,
        ActivityOverviewInput {
            agent: "agent-a".to_owned(),
            generated_at: "2026-05-20T10:00:00Z".to_owned(),
            refresh_interval_secs: 30,
            foreground: vec![],
        },
    )
    .unwrap();

    let run = &response.crons[0].recent_runs[0];
    assert_eq!(
        run.run_note.as_deref(),
        Some("Malformed delivery should not break overview.")
    );
    assert!(run.delivery_required);
    assert_eq!(run.delivery_status, "pending");
    assert!(run.delivery_kind.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
devenv shell -- cargo test -p right-dashboard activity_overview_
```

Expected: FAIL because `RunSummary` has no `run_note`, `delivery_required`, or `delivery_kind` fields.

- [ ] **Step 3: Extend the Rust DTO**

In `crates/right-dashboard/src/api_types.rs`, replace `RunSummary` with:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

- [ ] **Step 4: Select and parse the new fields**

In `crates/right-dashboard/src/read_model/activity.rs`, replace `RUN_SUMMARY_COLUMNS` with:

```rust
const RUN_SUMMARY_COLUMNS: &str =
    "ar.id, ar.kind, ar.producer_ref, ar.status, ar.started_at, ar.finished_at,
        ar.exit_code, ar.delivery_status, ar.delivery_required, ar.delivery_json,
        ar.run_note, costs.cost_usd";
```

Replace `run_summary_from_row` with:

```rust
fn run_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunSummary> {
    let delivery_json: Option<String> = row.get(9)?;
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
        delivery_kind: delivery_kind_from_json(delivery_json.as_deref()),
        run_note: row.get(10)?,
        cost_usd: row.get(11)?,
    })
}

fn delivery_kind_from_json(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| value.get("kind").and_then(|kind| kind.as_str()).map(str::to_owned))
}
```

In `activity_run_detail`, the selected columns shifted by three fields. Replace the tuple reads with:

```rust
Ok((
    run_summary_from_row(row)?,
    row.get::<_, Option<String>>(12)?,
    row.get::<_, Option<String>>(13)?,
    row.get::<_, Option<String>>(14)?,
    row.get::<_, Option<String>>(15)?,
))
```

- [ ] **Step 5: Run targeted read-model tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard activity_overview_
devenv shell -- cargo test -p right-dashboard activity_run_detail_parses_valid_delivery_json
```

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/activity.rs
git commit -m "feat(dashboard): expose cron run summary fields"
```

## Task 2: Add Dashboard API Coverage

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Write failing API assertion**

In `activity_overview_returns_current_cron_payload` in `crates/bot/src/telegram/dashboard.rs`, insert this `async_runs` row after the existing `cron_specs` insert:

```rust
conn.execute(
    "INSERT INTO async_runs (
        id, kind, producer_ref, run_session_id, target_chat_id,
        target_thread_id, status, started_at, finished_at, exit_code,
        run_note, delivery_json, delivery_required, delivery_status,
        created_at, updated_at
     ) VALUES (
        'run-1', 'cron', 'daily', 'run-1', 123, 456, 'success',
        '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z', 0,
        'Daily check completed.',
        '{\"kind\":\"notify\",\"content\":\"Daily check completed.\"}',
        1, 'pending',
        '2026-05-20T08:00:00Z', '2026-05-20T08:01:00Z'
     )",
    [],
)
.expect("insert cron run");
```

Add these assertions after the existing cron assertions:

```rust
assert_eq!(body["crons"][0]["recent_runs"][0]["id"], "run-1");
assert_eq!(
    body["crons"][0]["recent_runs"][0]["run_note"],
    "Daily check completed."
);
assert_eq!(
    body["crons"][0]["recent_runs"][0]["delivery_kind"],
    "notify"
);
assert_eq!(
    body["crons"][0]["recent_runs"][0]["delivery_required"],
    true
);
assert_eq!(
    body["crons"][0]["recent_runs"][0]["delivery_status"],
    "pending"
);
```

- [ ] **Step 2: Run test to verify it passes after Task 1**

Run:

```bash
devenv shell -- cargo test -p rightclaw-bot activity_overview_returns_current_cron_payload
```

Expected: PASS. If it fails with missing fields, Task 1 did not fully update serialization.

- [ ] **Step 3: Commit Task 2**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "test(dashboard): cover cron run summary API fields"
```

## Task 3: Add Frontend Delivery Helpers and Types

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/format.ts`

- [ ] **Step 1: Update TypeScript DTO**

In `crates/right-dashboard/frontend/src/types.ts`, replace `RunSummary` with:

```ts
export interface RunSummary {
  id: string
  kind: string
  producer_ref: string | null
  status: string
  started_at: string | null
  finished_at: string | null
  exit_code: number | null
  delivery_status: string
  delivery_required: boolean
  delivery_kind: string | null
  run_note: string | null
  cost_usd: number | null
}
```

- [ ] **Step 2: Add formatting helpers**

Append this to `crates/right-dashboard/frontend/src/format.ts`:

```ts
type DeliveryFields = {
  delivery_kind?: string | null
  delivery_required?: boolean
  delivery_status?: string | null
}

export function deliveryLabel(run: DeliveryFields): string {
  if (run.delivery_kind === 'notify') {
    return 'Notify'
  }
  if (run.delivery_kind === 'silent') {
    return 'Silent'
  }
  if (run.delivery_required) {
    return run.delivery_status ?? 'Pending'
  }
  return run.delivery_status ?? 'None'
}

export function deliveryTone(run: DeliveryFields): string {
  if (run.delivery_kind === 'notify' || run.delivery_status === 'pending' || run.delivery_status === 'retryable') {
    return 'active'
  }
  if (run.delivery_status === 'failed') {
    return 'bad'
  }
  if (run.delivery_status === 'delivered') {
    return 'ok'
  }
  return 'muted'
}

export function deliveryText(value: unknown): string | null {
  if (value === null || value === undefined) {
    return null
  }
  if (typeof value === 'object') {
    const record = value as Record<string, unknown>
    if (record.kind === 'notify' && typeof record.content === 'string') {
      return record.content
    }
    if (record.kind === 'silent' && typeof record.reason === 'string') {
      return record.reason
    }
  }
  return notifyText(value)
}
```

- [ ] **Step 3: Run frontend typecheck**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend run typecheck
```

Expected: PASS. This task only extends types/helpers, so no template changes are checked yet.

- [ ] **Step 4: Commit Task 3**

```bash
git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/format.ts
git commit -m "feat(dashboard): add cron delivery display helpers"
```

## Task 4: Render Inline Selected Run Detail

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- Modify: `crates/right-dashboard/frontend/src/App.vue`

- [ ] **Step 1: Update ActivityView imports**

In `crates/right-dashboard/frontend/src/views/ActivityView.vue`, replace the format import with:

```ts
import { deliveryLabel, deliveryText, deliveryTone, money, notifyText, shortDate, shortId, statusTone } from '../format'
```

- [ ] **Step 2: Render row badges, note preview, and inline detail**

In `crates/right-dashboard/frontend/src/views/ActivityView.vue`, replace the current `v-for="run in cron.recent_runs"` button block with:

```vue
<template v-for="run in cron.recent_runs" :key="run.id">
  <button
    class="data-row"
    :class="{ selected: selectedRunId === run.id }"
    type="button"
    @click="emit('selectRun', run)"
  >
    <span class="row-main">
      <span class="status-dot" :class="statusTone(run.status)"></span>
      <strong>{{ run.status }}</strong>
      <small>{{ shortId(run.id) }}</small>
      <span class="run-delivery-badge" :class="deliveryTone(run)">
        {{ deliveryLabel(run) }}
      </span>
      <small v-if="run.run_note" class="run-note-preview">{{ run.run_note }}</small>
    </span>
    <span class="row-side">
      <strong>{{ money(run.cost_usd) }}</strong>
      <small>{{ shortDate(run.started_at) }}</small>
    </span>
  </button>

  <section v-if="selectedRunId === run.id" class="run-inline-detail">
    <p v-if="loadingDetail" class="muted-line">Loading run detail</p>
    <p v-else-if="detailError" class="notice inline">{{ detailError }}</p>
    <template v-else-if="selectedRun">
      <dl class="meta-grid compact">
        <div>
          <dt>Delivery</dt>
          <dd>{{ deliveryLabel(selectedRun.run) }}</dd>
        </div>
        <div>
          <dt>Status</dt>
          <dd>{{ selectedRun.run.delivery_status }}</dd>
        </div>
        <div>
          <dt>Exit</dt>
          <dd>{{ selectedRun.run.exit_code ?? 'none' }}</dd>
        </div>
        <div>
          <dt>Finished</dt>
          <dd>{{ shortDate(selectedRun.run.finished_at) }}</dd>
        </div>
      </dl>
      <section class="text-block">
        <h3>Run note</h3>
        <p>{{ selectedRun.run_note || run.run_note || 'No run note' }}</p>
      </section>
      <section v-if="deliveryText(selectedRun.delivery)" class="text-block">
        <h3>Delivery</h3>
        <p>{{ deliveryText(selectedRun.delivery) }}</p>
      </section>
      <section v-else-if="notifyText(selectedRun.delivery)" class="text-block">
        <h3>Delivery</h3>
        <pre>{{ notifyText(selectedRun.delivery) }}</pre>
      </section>
      <section v-if="selectedRun.delivery_error" class="text-block">
        <h3>Delivery error</h3>
        <p>{{ selectedRun.delivery_error }}</p>
      </section>
      <section v-if="selectedRun.error_message" class="text-block">
        <h3>Error</h3>
        <p>{{ selectedRun.error_message }}</p>
      </section>
    </template>
    <template v-else>
      <section class="text-block">
        <h3>Run note</h3>
        <p>{{ run.run_note || 'No run note' }}</p>
      </section>
    </template>
  </section>
</template>
```

- [ ] **Step 3: Add CSS**

In `crates/right-dashboard/frontend/src/App.vue`, add this near the existing `.data-row` / `.row-main` CSS:

```css
.run-delivery-badge {
  min-height: 20px;
  padding: 2px 6px;
  border-radius: 999px;
  background: var(--tg-theme-secondary-bg-color, #fff);
  color: var(--tg-theme-hint-color, #546675);
  font-size: 0.7rem;
  font-weight: 750;
  text-transform: uppercase;
}

.run-delivery-badge.ok {
  color: #0d7a45;
  background: #dff5e8;
}

.run-delivery-badge.active {
  color: #8a5a00;
  background: #fff0c2;
}

.run-delivery-badge.bad {
  color: #a42323;
  background: #ffe1de;
}

.run-note-preview {
  flex-basis: 100%;
  display: -webkit-box;
  overflow: hidden;
  color: var(--tg-theme-hint-color, #6b7b88);
  line-height: 1.25;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

.run-inline-detail {
  display: grid;
  gap: 9px;
  padding: 9px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
}

.run-inline-detail .text-block:first-child {
  border-top: 0;
  padding-top: 0;
}
```

- [ ] **Step 4: Run frontend build**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS and rebuilt files under `crates/right-dashboard/static/dashboard/`.

- [ ] **Step 5: Commit Task 4**

```bash
git add crates/right-dashboard/frontend/src/views/ActivityView.vue crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/static/dashboard
git commit -m "feat(dashboard): inline cron run details"
```

## Task 5: Architecture Docs and Final Verification

**Files:**
- Check: `docs/architecture/modules.md`
- Check: `docs/architecture/sessions.md`
- Modify only if drift is found.

- [ ] **Step 1: Re-read architecture docs**

Run:

```bash
devenv shell -- sed -n '70,90p' docs/architecture/modules.md
devenv shell -- sed -n '180,195p' docs/architecture/sessions.md
```

Expected: `modules.md` already describes dashboard activity projections over async runs, cron specs, run notifications, and logs. `sessions.md` already describes `run_note` and `delivery_json`. If the implementation stayed within this behavior, do not edit these docs.

- [ ] **Step 2: Run targeted Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard activity_overview
devenv shell -- cargo test -p rightclaw-bot activity_overview_returns_current_cron_payload
devenv shell -- cargo test -p rightclaw-bot run_detail_returns_not_found_for_unknown_run
devenv shell -- cargo test -p rightclaw-bot activity_run_detail_returns_not_found_for_unknown_run
```

Expected: PASS.

- [ ] **Step 3: Run frontend build**

Run:

```bash
devenv shell -- npm --prefix crates/right-dashboard/frontend run build
```

Expected: PASS.

- [ ] **Step 4: Run final workspace verification**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If there are pre-existing failures, record the exact failing tests and confirm they are unrelated to the dashboard changes before stopping.

- [ ] **Step 5: Commit docs if changed**

If architecture docs changed:

```bash
git add docs/architecture/modules.md docs/architecture/sessions.md
git commit -m "docs(dashboard): document cron run detail display"
```

If docs did not change, skip this commit.
