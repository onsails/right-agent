# Learning Signal Severity & Detail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `refused` a neutral (non-alert) learning outcome, color `failed`/`aborted`/`created`/`updated` correctly, and surface the explanatory detail text in the Learning → Reports signal panel with a click-to-expand row.

**Architecture:** One shared backend function (`learning_outcome_severity`) feeds all three learning consumers (Overview signals, Overview cost-river markers, Learning recent-signals), so the taxonomy fix is a single edit. The frontend `statusTone` is taught the semantic severity levels so the corrected backend values color correctly. The `LearningSignalPanel` gains the detail field (already stored in the DB, previously discarded) and adopts the Overview `SignalTimeline` expand pattern.

**Tech Stack:** Rust (`right-dashboard` read-model, Turso/SQLite), Vue 3 + TypeScript, Vitest (vitest run, run via `pnpm`), SSR component tests via `@vue/server-renderer`.

---

## Pre-flight note (commit hooks)

This repo's `prek` git-hook shim is currently in **migration mode** and may abort `git commit` with `error: prek's Git shim is installed in migration mode`. If that happens, run once:

```bash
prek install -f --hook-type pre-commit
```

Then retry the commit normally (so `rustfmt` runs on Rust commits). Do **not** routinely use `--no-verify` for code commits.

## Baseline verification

- [ ] **Step 0: Establish a green baseline**

Run: `devenv shell -- cargo test -p right-dashboard`
Then: `cd crates/right-dashboard/frontend && pnpm vitest run`
Expected: both pass (record any pre-existing failures before starting).

---

## Task 1: Backend severity taxonomy

Single source of truth for all three learning-signal consumers.

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning_outcomes.rs` (`learning_outcome_severity`)
- Modify (tests): `crates/right-dashboard/src/read_model/dashboard_overview.rs:1223` (`dashboard_overview_projects_refused_learning_outcomes`)
- Modify (tests): `crates/right-dashboard/src/read_model/learning.rs:1069` (`learning_overview_recent_signals_parse_utc_bounds_and_sort`)

- [ ] **Step 1: Update the existing tests to the new expectations (failing-first)**

In `dashboard_overview.rs`, function `dashboard_overview_projects_refused_learning_outcomes`, change both `"warn"` expectations to `"info"`:

```rust
        assert!(response.signals.iter().any(|signal| {
            signal.kind == "learning_outcome"
                && signal.severity == "info"
                && signal.title == "Learning refused"
                && signal.related_skill_name.as_deref() == Some("rightx-refused")
        }));
        assert!(response.cost_learning_river.markers.iter().any(|marker| {
            marker.kind == "skill_refused"
                && marker.severity == "info"
                && marker.skill_name.as_deref() == Some("rightx-refused")
        }));
```

In `learning.rs`, function `learning_overview_recent_signals_parse_utc_bounds_and_sort`, the first signal is a `skill_updated` (status `"updated"`), which must now be `"ok"`:

```rust
        assert_eq!(response.recent_learning_signals[0].kind, "skill_updated");
        assert_eq!(response.recent_learning_signals[0].severity, "ok");
```

- [ ] **Step 2: Confirm no other test still asserts the old learning severities**

Run: `rg -n 'severity == "warn"|\.severity, "info"|\.severity, "warn"' crates/right-dashboard/src`
Expected: the only remaining `"warn"`/`"info"` learning-outcome assertions are ones you just edited or unrelated (e.g. curator/budget markers that hardcode `"warn"` directly, sandbox/health/active severities). If any other learning-outcome test asserts a stale value, update it to: `refused→info`, `failed`/`aborted`→`bad`, `created`/`updated`→`ok`.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `devenv shell -- cargo test -p right-dashboard dashboard_overview_projects_refused_learning_outcomes learning_overview_recent_signals_parse_utc_bounds_and_sort`
Expected: FAIL — current code emits `"warn"` for refused and `"info"` for updated.

- [ ] **Step 4: Rewrite the severity function**

In `learning_outcomes.rs`, replace `learning_outcome_severity` with:

```rust
pub(super) fn learning_outcome_severity(
    status: Option<&str>,
    hint_outcome: Option<&str>,
) -> &'static str {
    match (status, hint_outcome) {
        (_, Some("refused")) => "info",
        (Some("failed" | "aborted"), _) => "bad",
        (Some("created" | "updated"), _) => "ok",
        _ => "info",
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devenv shell -- cargo test -p right-dashboard dashboard_overview_projects_refused_learning_outcomes learning_overview_recent_signals_parse_utc_bounds_and_sort`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/read_model/learning_outcomes.rs \
        crates/right-dashboard/src/read_model/dashboard_overview.rs \
        crates/right-dashboard/src/read_model/learning.rs
git commit -m "fix(dashboard): refused learning outcome is info, not warn; align severity tones"
```

---

## Task 2: Backend — carry detail into the learning signal

Stop discarding `summary`/`message`; mirror the Overview `learning_outcome_signals` query.

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs:343` (`LearningSignalPoint`)
- Modify: `crates/right-dashboard/src/read_model/learning.rs:570` (`recent_learning_signals`)
- Test: `crates/right-dashboard/src/read_model/learning.rs` (new test in the existing `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `learning.rs` (it already provides `fixture()` and `input()` helpers used by sibling tests):

```rust
    #[tokio::test]
    async fn recent_learning_signals_include_detail_summary_then_message() {
        let (_dir, conn) = fixture().await;
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, hint_outcome, created_at
             ) VALUES
                ('s1', 'right', 'create', 'rightx-sum', 'finish', 'aborted',
                 'msg-a', 'summary-a', '[]', 'refused', '2026-05-20T10:00:00Z'),
                ('s2', 'right', 'create', 'rightx-msg', 'finish', 'aborted',
                 'msg-b', NULL, '[]', 'refused', '2026-05-20T09:00:00Z')",
            [],
        )
        .await
        .unwrap();

        let response = learning_overview(&conn, input()).await.unwrap();

        let sum = response
            .recent_learning_signals
            .iter()
            .find(|s| s.label == "rightx-sum")
            .expect("summary signal present");
        assert_eq!(sum.detail.as_deref(), Some("summary-a"));

        let msg = response
            .recent_learning_signals
            .iter()
            .find(|s| s.label == "rightx-msg")
            .expect("message-fallback signal present");
        assert_eq!(msg.detail.as_deref(), Some("msg-b"));
    }
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `devenv shell -- cargo test -p right-dashboard recent_learning_signals_include_detail_summary_then_message`
Expected: FAIL — `LearningSignalPoint` has no field `detail`.

- [ ] **Step 3: Add the `detail` field to the struct**

In `api_types.rs`, add `detail` after `severity` in `LearningSignalPoint`:

```rust
pub struct LearningSignalPoint {
    pub id: String,
    pub occurred_at: String,
    pub kind: String,
    pub label: String,
    pub severity: String,
    pub detail: Option<String>,
    pub skill_name: Option<String>,
    pub count: i64,
}
```

- [ ] **Step 4: Select and populate the detail in the query**

In `learning.rs::recent_learning_signals`, update the SQL, the `query_map` closure, the destructure, and the struct literal:

```rust
    let mut stmt = conn.prepare(
        "SELECT id, action, skill_name, status, hint_outcome, COALESCE(summary, message), created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
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
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .await?;
    let mut signals = Vec::<(DateTime<Utc>, i64, LearningSignalPoint)>::new();
    for row in rows {
        let (id, action, skill_name, status, hint_outcome, detail, occurred_at) = row?;
        let occurred_at_utc = parse_utc(&occurred_at)?;
        if occurred_at_utc < *since || occurred_at_utc > *now {
            continue;
        }
        signals.push((
            occurred_at_utc,
            id,
            LearningSignalPoint {
                id: format!("learning:{id}"),
                occurred_at: occurred_at_utc.to_rfc3339(),
                kind: learning_outcome_kind(&action, status.as_deref(), hint_outcome.as_deref())
                    .to_owned(),
                label: skill_name.clone(),
                severity: learning_outcome_severity(status.as_deref(), hint_outcome.as_deref())
                    .to_owned(),
                detail,
                skill_name: Some(skill_name),
                count: 1,
            },
        ));
    }
```

(Only the SELECT column list, the added `row.get::<_, Option<String>>(5)?` / shifted `created_at` to index 6, the `detail` binding in the destructure, and the `detail,` struct field are new. Everything else is unchanged.)

- [ ] **Step 5: Run the test to verify it passes**

Run: `devenv shell -- cargo test -p right-dashboard recent_learning_signals_include_detail_summary_then_message`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/learning.rs
git commit -m "feat(dashboard): include detail (summary/message) in learning signals"
```

---

## Task 3: Frontend — teach statusTone the severity levels

**Files:**
- Modify: `crates/right-dashboard/frontend/src/format.ts` (`statusTone`)
- Test: `crates/right-dashboard/frontend/src/format.test.ts`

- [ ] **Step 1: Write the failing tests**

Append to `format.test.ts` (ensure `statusTone` is in the import from `./format`):

```ts
describe('statusTone learning severity levels', () => {
  it('maps ok to the ok tone', () => {
    expect(statusTone('ok')).toBe('ok')
  })
  it('maps bad to the bad tone', () => {
    expect(statusTone('bad')).toBe('bad')
  })
  it('maps info to the muted tone', () => {
    expect(statusTone('info')).toBe('muted')
  })
  it('keeps warn on the active tone for other callers', () => {
    expect(statusTone('warn')).toBe('active')
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/format.test.ts`
Expected: FAIL — `statusTone('ok')` and `statusTone('bad')` currently return `'muted'`.

- [ ] **Step 3: Add the level recognition**

In `format.ts::statusTone`, add `'ok'` to the ok-branch and `'bad'` to the bad-branch. The ok-branch becomes:

```ts
  if (
    normalized === 'success' ||
    normalized === 'delivered' ||
    normalized === 'pass' ||
    normalized === 'configured' ||
    normalized === 'sandbox' ||
    normalized === 'host' ||
    normalized === 'host_mirror' ||
    normalized === 'create_candidate' ||
    normalized === 'update_candidate' ||
    normalized === 'ok'
  ) {
    return 'ok'
  }
  if (
    normalized === 'failed' ||
    normalized === 'fail' ||
    normalized === 'error' ||
    normalized === 'unavailable' ||
    normalized === 'bad'
  ) {
    return 'bad'
  }
```

(`'info'` needs no entry — it falls through to the default `'muted'`, which the test asserts.)

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/format.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/format.ts crates/right-dashboard/frontend/src/format.test.ts
git commit -m "fix(dashboard): statusTone recognizes ok/bad severity levels"
```

---

## Task 4: Frontend — type field + outcome-label helper

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts` (`LearningSignalPoint`)
- Create: `crates/right-dashboard/frontend/src/components/charts/learningSignalLabel.ts`
- Test: `crates/right-dashboard/frontend/src/components/charts/learningSignalLabel.test.ts`

- [ ] **Step 1: Add the `detail` field to the frontend type**

In `types.ts`, add `detail` after `severity` in `LearningSignalPoint`:

```ts
export interface LearningSignalPoint {
  id: string
  occurred_at: string
  kind: string
  label: string
  severity: string
  detail: string | null
  skill_name: string | null
  count: number
}
```

- [ ] **Step 2: Write the failing helper test**

Create `learningSignalLabel.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { learningSignalLabel } from './learningSignalLabel'

describe('learningSignalLabel', () => {
  it.each([
    ['skill_created', 'Created'],
    ['skill_updated', 'Updated'],
    ['skill_refused', 'Refused'],
    ['skill_failed', 'Failed'],
    ['skill_aborted', 'Aborted'],
    ['skill_learned', 'Learned'],
    ['something_unexpected', 'Learned'],
  ])('maps %s to %s', (kind, label) => {
    expect(learningSignalLabel(kind)).toBe(label)
  })
})
```

- [ ] **Step 3: Run to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/components/charts/learningSignalLabel.test.ts`
Expected: FAIL — module does not exist.

- [ ] **Step 4: Create the helper**

Create `learningSignalLabel.ts`:

```ts
export function learningSignalLabel(kind: string): string {
  switch (kind) {
    case 'skill_created':
      return 'Created'
    case 'skill_updated':
      return 'Updated'
    case 'skill_refused':
      return 'Refused'
    case 'skill_failed':
      return 'Failed'
    case 'skill_aborted':
      return 'Aborted'
    default:
      return 'Learned'
  }
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/components/charts/learningSignalLabel.test.ts`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts \
        crates/right-dashboard/frontend/src/components/charts/learningSignalLabel.ts \
        crates/right-dashboard/frontend/src/components/charts/learningSignalLabel.test.ts
git commit -m "feat(dashboard): learningSignalLabel helper + detail on signal type"
```

---

## Task 5: Frontend — expandable LearningSignalPanel with detail and outcome pill

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue`
- Test: `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.test.ts`

- [ ] **Step 1: Write the failing SSR test**

Create `LearningSignalPanel.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import LearningSignalPanel from './LearningSignalPanel.vue'
import type { LearningSignalPoint } from '../../types'

function signal(over: Partial<LearningSignalPoint> = {}): LearningSignalPoint {
  return {
    id: 'learning:1',
    occurred_at: '2026-05-20T10:00:00Z',
    kind: 'skill_refused',
    label: 'rightx-twitter-content-drafter',
    severity: 'info',
    detail: 'Insufficient evidence.',
    skill_name: 'rightx-twitter-content-drafter',
    count: 1,
    ...over,
  }
}

async function render(signals: LearningSignalPoint[]): Promise<string> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const app = createSSRApp({ render: () => h(LearningSignalPanel, { signals } as any) })
  return renderToString(app)
}

describe('LearningSignalPanel', () => {
  it('shows the detail preview text', async () => {
    const html = await render([signal()])
    expect(html).toContain('Insufficient evidence.')
  })

  it('shows a human outcome label instead of a raw severity word', async () => {
    const html = await render([signal({ kind: 'skill_refused' })])
    expect(html).toContain('Refused')
  })

  it('colors a refused signal muted, never the active (amber) alert tone', async () => {
    const html = await render([signal({ severity: 'info' })])
    const tone = html.match(/class="status-pill ([a-z]+)"/)
    expect(tone?.[1]).toBe('muted')
  })

  it('renders rows as interactive buttons (collapsed by default)', async () => {
    const html = await render([signal()])
    expect(html).toContain('<button')
    expect(html).toContain('aria-expanded="false"')
  })

  it('shows the empty state when there are no signals', async () => {
    const html = await render([])
    expect(html).toContain('No recent learning outcomes')
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/components/charts/LearningSignalPanel.test.ts`
Expected: FAIL — current panel renders a non-interactive `div.data-row.static`, no `detail`, no outcome label.

- [ ] **Step 3: Rewrite the panel**

Replace the entire contents of `LearningSignalPanel.vue` with:

```vue
<script setup lang="ts">
import { ref } from 'vue'

import StatusPill from '../StatusPill.vue'
import { shortDate } from '../../format'
import { learningSignalLabel } from './learningSignalLabel'
import type { LearningSignalPoint } from '../../types'

defineProps<{
  signals: LearningSignalPoint[]
}>()

const selectedId = ref<string | null>(null)

function toggle(id: string): void {
  selectedId.value = selectedId.value === id ? null : id
}
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
      <template v-for="signal in signals" :key="signal.id">
        <button
          type="button"
          class="data-row"
          :class="{ selected: selectedId === signal.id }"
          :aria-expanded="selectedId === signal.id"
          @click="toggle(signal.id)"
        >
          <span class="row-main">
            <strong>{{ signal.label }}</strong>
            <span v-if="signal.detail" class="signal-preview">{{ signal.detail }}</span>
            <small>
              {{ shortDate(signal.occurred_at) }}
              <template v-if="signal.skill_name"> / {{ signal.skill_name }}</template>
              <template v-if="signal.count > 1"> / {{ signal.count }}</template>
            </small>
          </span>
          <span class="row-side">
            <StatusPill :status="signal.severity" :label="learningSignalLabel(signal.kind)" />
          </span>
        </button>
        <dl v-if="selectedId === signal.id" class="meta-grid compact signal-detail">
          <div><dt>When</dt><dd>{{ shortDate(signal.occurred_at) }}</dd></div>
          <div><dt>Kind</dt><dd>{{ signal.kind }}</dd></div>
          <div v-if="signal.skill_name"><dt>Skill</dt><dd>{{ signal.skill_name }}</dd></div>
          <div v-if="signal.detail" class="signal-detail-text"><dt>Detail</dt><dd>{{ signal.detail }}</dd></div>
        </dl>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.signal-preview {
  display: block;
  opacity: 0.85;
}
.signal-detail {
  padding: 8px 12px 14px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 0 0 10px 10px;
  margin-bottom: 8px;
}
.signal-detail-text {
  grid-column: 1 / -1;
}
</style>
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run src/components/charts/LearningSignalPanel.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue \
        crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.test.ts
git commit -m "feat(dashboard): expandable learning signal rows with detail and outcome label"
```

---

## Task 6: Final verification

- [ ] **Step 1: Frontend suite**

Run: `cd crates/right-dashboard/frontend && pnpm vitest run`
Expected: PASS (including the existing `ReportsView` SSR test, which embeds `LearningSignalPanel`).

- [ ] **Step 2: Lint/type-check the frontend (if configured)**

Run: `cd crates/right-dashboard/frontend && pnpm run lint`
Expected: PASS. (If no `lint` script exists, skip.)

- [ ] **Step 3: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Investigate any failure before declaring complete; re-run a suspected-flaky test in isolation before blaming this change.

---

## Self-review notes (for the implementer)

- **Spec coverage:** Task 1 = severity taxonomy (refused→info, failed/aborted→bad, created/updated→ok, all three consumers via the shared fn). Task 2 = backend detail. Task 3 = tone fix. Task 4 = type + outcome-label helper. Task 5 = panel expand + detail + label. Task 6 = verification cadence.
- **Shared-function reach:** `learning_outcome_severity` has exactly three callers (Overview signals, Overview cost-river markers, Learning recent-signals). Task 1 Step 2 grep guards against a missed test expectation.
- **No flow-chart regression:** `LearningFlowChart.vue` does not call `statusTone`, and no current `StatusPill` caller passes a literal `ok`/`bad`/`info` except the learning severity pills — so Task 3 recolors only the intended targets.
- **Expand interaction:** the toggle is local presentational state (`selectedId`), no container plumbing and no fetch — `detail` is already in the payload. SSR tests assert the collapsed render (preview, button, tone, label, empty state); the expand branch is trivial local state.
