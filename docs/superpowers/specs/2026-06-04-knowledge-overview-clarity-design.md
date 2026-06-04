# Knowledge & Overview clarity redesign

**Date:** 2026-06-04
**Agent that surfaced it:** `agent-b`
**Status:** design, awaiting implementation plan

## Problem

The Overview tab renders a row of marker chips under the cost chart. Each
chip is one finished learning event, labelled with the raw skill slug
(`rightx-obsidian-vault-sync`). For `agent-b` the same skill appeared seven
times in a row, reading as a meaningless list of "cron names".

Investigating the data exposed a deeper defect than the chips:

| Skill | Action | Outcome | Count |
|---|---|---|---|
| `rightx-obsidian-vault-sync` | create | created | 1 |
| `rightx-obsidian-vault-sync` | update | updated | 1 |
| `rightx-obsidian-vault-sync` | update | **aborted / refused** | **5** |
| `rightx-notion-task-agenda` | create | created | 1 |

Five of the seven `obsidian-vault-sync` events were **refusals**: the
probe-writer looked at a hint, decided the skill already covered the
request, and changed nothing. The dashboard gave those no-ops the same
weight and appearance as real edits.

The same conflation lives in the Knowledge → Learning subtab. Both
`recent_failed_events` and the `failed_or_aborted_7d` counter filter on
`status IN ('failed','aborted')` and ignore `hint_outcome`. Refusals
carry `status='aborted'` + `hint_outcome='refused'`, so they leak into
"Failed skills" and inflate the "Failed 7d" card. For `agent-b` this turns
five healthy no-ops into "five failures".

A genuine failure means a learning attempt errored out. A refusal means
the writer deliberately declined because the skill already covered the
request. These are opposite outcomes sharing one bucket.

Secondary Knowledge-subtab layout faults:

- Learning signals sits beside the tall Sankey chart; the short right
  column leaves dead space under the graph.
- "Failed skills" is a full-width collapsible hidden behind a click on
  the "Failed 7d" card, disconnected from the signals list.
- Failed-skill rows are static `<div>`s, so clicking one reveals nothing.
- Nothing tells the user what "failed skill" means.

## Goals

1. Stop the Overview cost chart from surfacing learning events.
2. Make "failed" honest: separate genuine failures from refusals
   everywhere they are counted or listed.
3. Surface refusals as a low-key, clearly-labelled signal — not as
   failures.
4. Rework the Learning subtab so signals and failed skills sit side by
   side with no dead space, both always visible and expandable.
5. Add a one-line explanation of "failed skill".

Non-goals: redesigning the Sankey flow chart, the Skills subtab, or the
cost river itself; changing the learning pipeline.

## Design

### 1. Overview — drop the learning-marker overlay

Remove from the Overview cost chart both the marker chips
(`CostLearningRiver.vue` marker-list) and the scatter "pin" overlay, plus
the marker-detail block in `OverviewView.vue`. The chart becomes a pure
cost theme-river.

**Assumption (stated for review):** the user asked to remove "these
chips"; pins render the same raw slugs and serve the same events, so both
go. Cost-spike *signals* still appear in the Signal Timeline below, so
spikes are not lost.

Backend: stop assembling learning/curator markers in the Overview read
model (`dashboard_overview.rs`) so the response carries no dead marker
payload. The curator cost-spike **signal** stays; only its duplicate
*marker* is dropped. The shared `CostLearningRiver` API type keeps its
`markers` field (still used by type definitions/tests) but the Overview
path returns it empty — confirm during planning whether any other caller
populates it; if not, drop the field.

### 2. Backend — honest failure classification (`learning.rs`)

`learning_events_in_window` currently takes a two-status array and
filters `status IN (?,?)` without reading `hint_outcome`. Split the
failure path:

- **Genuine failures** (Failed skills list + `failed_7d` count):
  `status='failed'` OR (`status='aborted'` AND
  `COALESCE(hint_outcome,'') <> 'refused'`).
- **Refusals** (new, muted line): `status='aborted'` AND
  `hint_outcome='refused'`. Expose `refused_7d` count and a capped
  sample (`recent_refused_events`) reusing `LearningEventSummary`.

API/type changes (`api_types.rs` + frontend `types.ts`):

- Rename `failed_or_aborted_7d` → `failed_7d` (the value's meaning
  changes; a rename keeps maintainers honest). Update `failureMetric`
  call sites and tests.
- Add `refused_7d: i64` and `recent_refused_events: LearningEventSummary[]`
  to the learning lifecycle payload.

Implementation note: the shared window query must also SELECT
`hint_outcome` so callers can classify. Keep `recent_successful_events`
(`created`/`updated`) unchanged.

### 3. Learning subtab layout (`ReportsView.vue`)

```
┌─────────────────────────────────────────────┐
│  Learning flow (Sankey) — full width         │
├──────────────────┬──────────────────────────┤
│  Created │ Updated │ Failed   (7d cards)     │
├──────────────────┬──────────────────────────┤
│  Learning signals │  Failed skills           │  ← side by side, always visible
│  (expandable)     │  (now expandable)        │
├──────────────────┴──────────────────────────┤
│  muted: "Refused N — skill already covered   │
│         the request; nothing changed."       │
└──────────────────────────────────────────────┘
```

- Move the Sankey chart to its own full-width row.
- Keep the three lifecycle cards (Created / Updated / Failed 7d). The
  "Failed 7d" card stops being an interactive toggle — the failed list is
  always visible now, so the card is a plain number.
- Place `LearningSignalPanel` and a panel-wrapped `FailedSkillList`
  side by side in a `two-column` section, both always rendered.
- Below them, a full-width muted caption line for refusals, shown only
  when `refused_7d > 0`. Wording: "Refused {refused_7d} — the skill
  already covered the request; nothing changed." Clicking is out of
  scope; a plain caption suffices.

Honour the dashboard frontend primitives rule: loading/empty/error stay
inside `AsyncState`; the muted line is content, not a placeholder.

### 4. Failed skills — expandable rows + explainer (`FailedSkillList.vue`)

- Wrap the list in a `panel` with a header (`eyebrow` "Failed skills",
  `h2` "Recent failures") so it balances `LearningSignalPanel`
  side-by-side.
- Convert rows from `<div class="data-row static">` to expandable
  `<button class="data-row">` mirroring `LearningSignalPanel`: a
  `selectedKey` ref toggles a `<dl>` detail block showing status, action,
  when, full `message`, and `summary`.
- Add a short explainer next to the header (info line or `?` affordance):
  "A failed skill is a learning attempt that errored out. It is not a
  refusal — a refusal means the skill already covered the request."
  Reuse an existing tooltip/info atom if one exists; otherwise a muted
  caption line under the header.

## Affected files

Backend:
- `crates/right-dashboard/src/read_model/learning.rs` — split failed vs
  refused; add refused count + sample; SELECT `hint_outcome`.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — stop
  emitting learning/curator markers for the Overview river.
- `crates/right-dashboard/src/api_types.rs` — rename `failed_or_aborted_7d`
  → `failed_7d`; add `refused_7d`, `recent_refused_events`.

Frontend:
- `views/OverviewView.vue` — drop marker-detail block + select-marker
  plumbing.
- `components/charts/CostLearningRiver.vue` — drop marker chips, scatter
  pins, marker tooltip/label code.
- `views/learning/ReportsView.vue` — new layout; refusals caption; cards
  no longer toggle.
- `components/FailedSkillList.vue` — panel wrapper, expandable rows,
  explainer.
- `types.ts` — `failed_7d`, `refused_7d`, `recent_refused_events`.

Tests:
- `learning.rs` unit tests: assert refusals excluded from failed, counted
  in refused.
- `dashboard_overview.rs` tests: assert Overview river carries no markers.
- `FailedSkillList` / `ReportsView` SSR tests: side-by-side render,
  expandable failed rows, refusals caption visibility.
- Update any test referencing `failed_or_aborted_7d`.

## Verification

- Targeted: `devenv shell -- cargo test -p right-dashboard` after backend
  changes; `npm test` (or project equivalent) for the dashboard frontend
  after each component change.
- Final: `devenv shell -- cargo test --workspace`.
- Manual against `agent-b`: Overview chart shows no chips/pins; Learning
  subtab shows "Failed 7d = 0" and a "Refused 5 …" caption; a failed
  skill (once one exists) expands on click.
