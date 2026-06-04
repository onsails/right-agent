# Learning signal severity & detail — design

**Date:** 2026-06-04
**Status:** Approved (design), pending implementation plan

## Problem

In the dashboard **Learning → Reports → "Learning signals / Recent
outcomes"** panel, learned-skill outcomes show a `severity` badge. Two
defects compound there:

1. **`refused` is flagged amber.** When the probe-writer proposes a skill
   create/update and the agent declines it (`skill_learning_events.hint_outcome
   = 'refused'`), that is a routine, expected outcome — not a problem. Yet
   `learning_outcome_severity` maps `refused → "warn"`, which renders as the
   amber `active` tone: a false alarm.

2. **The badge has nothing behind it.** `LearningSignalPanel.vue` renders the
   row as a non-interactive `<div class="data-row static">` and
   `LearningSignalPoint` carries no detail field, so clicking reveals nothing.
   The explanatory text exists in the DB (`skill_learning_events.message` /
   `.summary`) but `recent_learning_signals` never selects it. The sibling
   Overview feed (`SignalTimeline.vue` over `DashboardSignal`) already selects
   `COALESCE(summary, message)` and renders it — this panel was left behind.

### Root cause of the inverted colors

`learning_outcome_severity` emits the vocabulary `warn` / `bad` / `info`, but
the frontend `statusTone` (format.ts) maps **status** words
(`success`/`failed`/`error`/`warn`/…), not severity levels. It does not know
`bad` or `info`, so both fall through to the `muted` (grey) tone. The only
severity that lands on an attention tone is `warn` (→ `active`). Net effect,
today:

| Outcome | severity (backend) | statusTone result | renders as |
|---|---|---|---|
| `refused` (routine) | `warn` | `active` | 🟠 amber "alert" |
| `failed` / `aborted` (real problem) | `bad` | unmatched → `muted` | ⚪ grey |
| `created` / `updated` (success) | `info` | unmatched → `muted` | ⚪ grey |

The routine case shouts; real failures are silent. Both severity panels
(Overview `SignalTimeline` and Learning `LearningSignalPanel`) feed `severity`
through the same `StatusPill` → `statusTone`, so a single backend fix corrects
both.

## Goals

- `refused` is a neutral, low-key outcome — never an amber alert.
- `failed` / `aborted` are visibly surfaced (bad/red tone).
- `created` / `updated` read as success (ok/green tone).
- The Learning signal panel shows the explanatory detail text and honors the
  click-to-expand instinct, consistent with the Overview `SignalTimeline`.

## Non-goals

- Unifying `SignalTimeline` and `LearningSignalPanel` into one component. Their
  data shapes (`DashboardSignal` vs `LearningSignalPoint`) differ; this is a
  separate refactor, out of scope.
- Changing the Overview `SignalTimeline` badge **text** (it shows the severity
  word but already conveys the outcome in `signal.title`). Only its **tone**
  changes, automatically, via the shared severity fix.
- Touching the `learning.warnings` "Partial data" notice — that is a separate,
  correctly-working data-fetch warning surface.

## Design

### 1. Backend — severity taxonomy

`crates/right-dashboard/src/read_model/learning_outcomes.rs`,
`learning_outcome_severity` (shared by Overview and Learning):

```rust
match (status, hint_outcome) {
    (_, Some("refused"))             => "info",  // was "warn" — no longer an alert
    (Some("failed" | "aborted"), _)  => "bad",   // unchanged — now actually colored
    (Some("created" | "updated"), _) => "ok",    // was caught by the "info" arm
    _ => "info",
}
```

This is the single source for both signal feeds.

### 2. Backend — detail in the learning signal

- Add `detail: Option<String>` to `LearningSignalPoint`
  (`crates/right-dashboard/src/api_types.rs`) and the frontend type
  (`types.ts`).
- In `crates/right-dashboard/src/read_model/learning.rs::recent_learning_signals`,
  extend the `SELECT` to include `COALESCE(summary, message)` and populate
  `detail`. Mirror the existing pattern in
  `dashboard_overview.rs::learning_outcome_signals` (the `COALESCE(summary,
  message)` column). No new data — stop discarding what is already stored.

### 3. Frontend — fix the tone mapping

`crates/right-dashboard/frontend/src/format.ts::statusTone`: teach it the
semantic severity levels so the backend can emit clean levels and they color
correctly:

- `'ok'` → `ok` tone
- `'bad'` → `bad` tone
- `'info'` → `muted` tone (explicit; matches current default)

Leave `'warn'` → `active` for any other callers. During implementation, verify
`LearningFlowNode.severity` values are not recolored unintentionally by these
additions.

### 4. Frontend — `LearningSignalPanel.vue` (expand pattern, option A)

- Row becomes a `<button>` with a local `selectedId` ref. Expansion is purely
  presentational: `detail` is already in the payload, so the click triggers no
  fetch and needs no container plumbing.
- An always-visible preview line shows `detail`.
- Clicking toggles an inline `<dl>` with the full `detail`, `kind`, and
  `occurred_at`, following `SignalTimeline.vue`'s expand markup.
- The pill shows `:status="signal.severity"` (drives tone) plus
  `:label="learningSignalLabel(signal.kind)"` (human outcome word). `StatusPill`
  already supports `label ?? status`.
- New pure helper `learningSignalLabel(kind)` (extracted `*.ts`, unit-tested per
  the dashboard convention):

  | kind | label |
  |---|---|
  | `skill_created` | Created |
  | `skill_updated` | Updated |
  | `skill_refused` | Refused |
  | `skill_failed` | Failed |
  | `skill_aborted` | Aborted |
  | `skill_learned` (fallback) | Learned |

  Without this the refused pill would read "INFO" — meaningless. With it the
  outcome is legible at a glance, colored by severity.

## Testing

TDD — write the failing test first where practical.

- **Update** `dashboard_overview_projects_refused_learning_outcomes`
  (`read_model/dashboard_overview.rs`): assert `severity == "info"` for the
  refused fixture (was `"warn"`). Add/adjust assertions so `created`/`updated`
  → `"ok"` and `failed`/`aborted` → `"bad"`.
- **New** backend test: `recent_learning_signals` returns `detail` populated
  from `summary` (and falls back to `message`).
- **New** unit test for `learningSignalLabel` covering every `kind` and the
  fallback.
- **New/updated** SSR test for `LearningSignalPanel.vue`: detail preview
  rendered, click expands full detail, refused pill is not the `active` tone.

## Verification cadence

- Targeted during work: `devenv shell -- cargo test -p right-dashboard
  learning` and the relevant frontend unit/SSR tests.
- Final, mandatory before declaring complete: `devenv shell -- cargo test
  --workspace` plus the dashboard frontend test suite.

## Affected files

- `crates/right-dashboard/src/read_model/learning_outcomes.rs` — severity map.
- `crates/right-dashboard/src/read_model/learning.rs` — `recent_learning_signals`
  query + `detail`.
- `crates/right-dashboard/src/api_types.rs` — `LearningSignalPoint.detail`.
- `crates/right-dashboard/src/read_model/dashboard_overview.rs` — test update.
- `crates/right-dashboard/frontend/src/format.ts` — `statusTone` levels.
- `crates/right-dashboard/frontend/src/types.ts` — `LearningSignalPoint.detail`.
- `crates/right-dashboard/frontend/src/components/charts/LearningSignalPanel.vue`
  — expand + pill label.
- New `learningSignalLabel` helper `*.ts` + its unit test, plus the panel SSR
  test.
