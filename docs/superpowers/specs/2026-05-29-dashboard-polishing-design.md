# Dashboard polishing — design

Date: 2026-05-29
Status: approved for planning

## Problem

The Telegram Mini App dashboard (`crates/right-dashboard` frontend, Vue 3 +
Vite) has accumulated five rough edges plus two cross-cutting ones:

1. **Overview order** — Cost & Learning sits at the bottom; it should lead.
2. **Signal detail scroll** — selecting a signal renders its detail far below
   the (long) list on the narrow phone layout, forcing a scroll to the bottom
   and losing the user's place in the list.
3. **Usage noise** — "Partial data … included unknown usage source
   `learning_reviewer` … `learning_selector`" warnings. These are **legacy
   rows** in `usage_events` from a removed learning pipeline; nothing in the
   current code writes them.
4. **Identity confusion** — raw enum codes (`MIXED`, `HOST_MIRROR`), a doubled
   file list, and an alarmist "IDENTITY.md unavailable in sandbox; …" banner
   that makes a normal empty state look like a malfunction.
5. **Skills** — `core` / `learned` / `other` groups are always fully expanded;
   they should collapse by default and show a count.
6. **Loading flash** — every tab briefly renders `'not loaded'` /
   `'unavailable'` / `'No X data'` text while its first fetch is in flight.
7. **DRY** — these placeholders and patterns are duplicated across views with
   no shared component.

## Key findings (grounding)

- **Identity is inlined into the system prompt, read from `/sandbox`.** The
  prompt-assembly script (`crates/bot/src/cc/prompt.rs:47-117`) emits, per file
  in `PROMPT_SECTIONS`:

  ```sh
  if [ -f /sandbox/IDENTITY.md ]; then
    printf '\n## Your Identity\n'
    cat /sandbox/IDENTITY.md
    printf '\n'
  fi
  ```

  `root_path` is `/sandbox` for sandboxed agents, the host `agent_dir` for
  `sandbox: mode: none`. A missing file is silently omitted by the `[ -f ]`
  gate — no identity, no error. Bootstrap mode skips identity files entirely
  (the agent is authoring them that turn).

- **Identity files are never uploaded host→sandbox.** They are excluded from
  sandbox staging (`crates/right-openshell/src/openshell.rs:1477,1510`) and from
  the platform-store manifest (`crates/right-platform-store/src/lib.rs:80`). The
  agent authors them inside `/sandbox`; the host copy is a debug/rebootstrap
  **mirror** synced *from* the sandbox
  (`crates/right-agent/src/identity_mirror.rs:3-8`).

- **Conclusion:** the dashboard already reads the same `/sandbox` path the
  prompt reads, so it is *aligned on the path*. What is misaligned is the
  **fallback**: when `/sandbox/<file>` is absent (a legitimate "agent hasn't
  authored identity yet" state), the dashboard silently substitutes the host
  mirror and stamps it `MIXED` / "unavailable in sandbox". This is a dashboard
  presentation defect, **not** a platform bug. Backend logic lives in
  `crates/bot/src/telegram/dashboard/identity.rs` and
  `crates/right-dashboard/src/identity_files.rs`.

- **Legacy usage sources are dead data.** `right_agent::usage::LEARNING_SOURCES`
  (`crates/right-agent/src/usage/mod.rs:23-27`) holds only `learning_prefilter`,
  `learning_probe_writer`, `learning_curator`. No code writes `learning_reviewer`
  or `learning_selector`; the dashboard `SOURCES` const
  (`crates/right-dashboard/src/read_model/usage.rs:12-19`) matches the live set.
  The rows are residue from a removed pipeline. The "unknown source" warning
  (`usage.rs:440-478`) is firing correctly but on stale data.

- **Loading model.** Views receive `null` data props until the first fetch
  resolves (`App.vue` refresh functions). `connectionState` starts `'loading'`.
  Each view falls back to literal placeholder text in the meantime
  (`HealthView.vue:31,71`; `IdentityView.vue:25`; `SkillsView.vue:94`;
  `chart-empty` in the chart components).

## Decisions

- Signal detail interaction: **inline accordion**, single-open, tapped row
  scroll-anchored (chosen over a bottom-sheet/dock).
- Legacy usage rows: **deleted** from the database (irreversible; lowers
  historical totals by the retired spend — accepted by the user).
- Identity: **dashboard-only fix**; no platform change. Distinguish
  authored / not-authored / unreachable instead of collapsing them.

## Solution

### A. Shared frontend primitives (build first — everything consumes them)

New components under `crates/right-dashboard/frontend/src/components/`:

- **`Spinner.vue`** — small indeterminate spinner atom, themed with the
  Telegram CSS custom properties already in use.
- **`AsyncState.vue`** — wraps a panel body. Props
  `{ loading: boolean, error: string | null, empty: boolean, emptyText?: string }`.
  Renders, in priority order: error notice (`error`), spinner
  (`loading && empty`), `emptyText` (`empty && !loading`), else the default
  slot. This is the single sanctioned way to render loading/empty/error and
  **replaces every scattered placeholder**.
- **`CollapsibleSection.vue`** — header with title slot + count badge +
  chevron; collapsible body slot; `defaultOpen?: boolean` prop (default
  `false`). Animated height toggle. Used by Skills groups; reusable elsewhere.

These are unit-tested with Vitest (render states for `AsyncState`, toggle for
`CollapsibleSection`).

### B. Overview (`views/OverviewView.vue`)

- Reorder template top→bottom: **`CostLearningRiver`**, then `metric-grid`,
  then the signals section.
- Replace the side-panel signal detail with an **inline accordion** on the
  narrow layout: `SignalTimeline` rows become expandable; tapping a row toggles
  an inline detail block beneath it (single-open). Use CSS scroll-anchoring (or
  an explicit anchor on toggle) so the tapped row does not jump. The wide-screen
  two-column layout is preserved via a breakpoint. The detail fields (when,
  source, skill, cost, kind, run/report, detail text) move into the expanded
  block.
- Marker selection (from the river chart) continues to populate the same detail
  surface.

### C. Usage (`right-db` migration + read model)

- New idempotent migration in `right_db::migrations::MIGRATIONS`:
  `DELETE FROM usage_events WHERE source IN ('learning_reviewer','learning_selector');`
  Idempotent by construction (re-running deletes zero rows). Follows the
  migration-ownership and single-transaction rules in `ARCHITECTURE.md`.
- No change to the unknown-source warning code — it stays as a guard for
  genuinely-unexpected future sources. With the rows gone, the "Partial data"
  banner disappears from Overview and Usage naturally.
- Existing `usage_overview_sources_match_learning_sources_constant` test must
  still pass.

### D. Identity (`dashboard/identity.rs` + `IdentityView.vue`)

**Root cause of the user's report (confirmed by live investigation).** Both
agents' identity files are present and healthy in their sandboxes (byte-matching
the host mirrors); no recreate/migration occurred. The "MIXED / unavailable in
sandbox" the user saw came from a **transient sandbox-exec timeout**, not missing
files: `read_sandbox_identity_files` (`identity.rs:97-134`) issues **three
sequential `exec_in_sandbox` calls**, one per file, each with a **4s timeout**
(`DASHBOARD_SANDBOX_TIMEOUT_SECS = 4`, `dashboard.rs:30`). On a cold/slow
sandbox these time out, and the timeout branch (`identity.rs:111-117`) emits the
*same* "unavailable in sandbox" warning as the genuine file-missing branch
(`:121-126`) — a slow read mislabeled as absent. Hence two backend fixes below in
addition to the relabeling.

Backend (`crates/bot/src/telegram/dashboard/identity.rs`):

- **Coalesce the three sequential reads into one `exec_in_sandbox`.** Run a
  single `sh -c` that emits all three files with unambiguous delimiters (and a
  per-file present/absent marker), then parse the combined output. One 4s-budget
  round-trip instead of three — ~3× faster and removes the dominant cause of the
  spurious timeout. Keep `read_sandbox_identity_file` (single-file path used by
  `identity_file_response`) consistent with the new state mapping.
- **Distinguish timeout from absence.** A timeout / gRPC error maps to
  `sandbox_unreachable`; only a real per-file "absent" marker maps to
  `not_authored` / `host_mirror`. The two must no longer share a warning string.
- Replace the collapsed `mixed` + "unavailable in sandbox" outcome with a
  per-file state machine:
  - sandbox read ok → **`sandbox`** (Live).
  - sandbox exec error / timeout → **`sandbox_unreachable`**.
  - sandbox file absent (exit 3), host mirror present → **`host_mirror`**
    (debug copy exists, not the live one).
  - sandbox file absent, host mirror absent → **`not_authored`**.
  - no-sandbox agent, host file present → **`host`** (authoritative).
  - no-sandbox agent, host file absent → **`missing`**.
- Surface per-file state; reserve a warning-tone banner **only** for
  `sandbox_unreachable`. `not_authored` / `host_mirror` are calm.

Frontend (`views/IdentityView.vue`):

- **Dedup**: one row per file that is both the selector and the status pill;
  remove the separate `meta-grid` duplicate listing.
- **Human labels** via `format.ts` `statusTone` + a label map keyed on the
  six states: `sandbox`→`Live`, `not_authored`→`Not authored yet`,
  `host_mirror`→`Host mirror`, `sandbox_unreachable`→`Sandbox unreachable`,
  `host`→`Host`, `missing`→`Missing`.
- Drop the `MIXED` overall pill and the semicolon-joined warning string; derive
  a single clear banner from the worst per-file state.
- For `sandbox_unreachable`, show a **retry** affordance (re-fetch identity)
  rather than presenting the host mirror as if it were the agent's live state.
- Use `AsyncState` for the detail panel loading/empty/error.

### E. Skills (`views/SkillsView.vue`)

- Wrap each of the three group panels (`core` / `learned` / `other`) in
  `CollapsibleSection`, `defaultOpen=false`, header count = number of skills in
  the group. Detail panel and pin behavior unchanged; route its
  loading/empty/error through `AsyncState`.

### F. Cross-cutting loading states

- Replace literal `'not loaded'` / `'unavailable'` / `'No X data'` / inline
  `Loading` text in `OverviewView`, `UsageView`, `IdentityView`, `SkillsView`,
  `HealthView`, `ActivityView` (and chart empty states where they front a
  not-yet-loaded fetch) with `AsyncState`, driven by the existing `connectionState`
  / `loading*` flags and `null` data props.

### G. ARCHITECTURE.md — Dashboard frontend section

Add a short **prescriptive** subsection (passes rule/enforcement/brevity tests):

> **Dashboard frontend primitives.** Loading/empty/error rendering MUST go
> through `AsyncState.vue`; collapsible grouped lists MUST use
> `CollapsibleSection.vue`. Raw placeholder text (`'not loaded'`,
> `'unavailable'`, ad-hoc `v-if="loading"` Loading lines) in a view is a
> review-blocking defect.

Keep ARCHITECTURE.md under its 40k budget — if the addition would exceed it,
move descriptive detail to a `docs/architecture/` satellite and link by plain
path.

## Out of scope

- The user's live agents were investigated (read-only) and are healthy:
  `him` and `right` both have all three identity files present in-sandbox,
  byte-matching the host mirrors, with no recreate/migration in the logs. The
  "missing" report was a transient exec timeout (addressed by §D), not data
  loss — so no platform/data-recovery work is needed.
- Any change to identity deployment, the prompt system, or sandbox staging.
- Tuning `DASHBOARD_SANDBOX_TIMEOUT_SECS` itself — the coalesced single-read in
  §D is the fix; raising the constant is a fallback only if the coalesced read
  still times out in practice (revisit then, not now).
- Restyling charts or the broader visual theme.

## Verification cadence

- During build (targeted): `cargo test -p right-db <migration filter>`,
  `cargo test -p right-dashboard`, `cargo test -p bot identity` (the coalesced
  sandbox-read parser + state mapping), and in `frontend/`: `pnpm test` +
  `pnpm typecheck`. TDD red/green for the migration (assert the two sources are
  gone and the migration is idempotent), for the identity read parser (a
  single combined-output read yields correct per-file present/absent +
  timeout→`sandbox_unreachable` mapping), and for `AsyncState` /
  `CollapsibleSection`.
- Final (mandatory): `devenv shell -- cargo test --workspace` plus a frontend
  `pnpm build`.

## Affected files (anticipated)

New:
- `crates/right-dashboard/frontend/src/components/Spinner.vue`
- `crates/right-dashboard/frontend/src/components/AsyncState.vue`
- `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue`
- migration entry in `crates/right-db/src/migrations/` (per registry convention)

Modified:
- `crates/right-dashboard/frontend/src/views/OverviewView.vue`
- `crates/right-dashboard/frontend/src/views/UsageView.vue`
- `crates/right-dashboard/frontend/src/views/IdentityView.vue`
- `crates/right-dashboard/frontend/src/views/SkillsView.vue`
- `crates/right-dashboard/frontend/src/views/HealthView.vue`
- `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue`
- `crates/right-dashboard/frontend/src/format.ts`
- `crates/right-dashboard/frontend/src/types.ts` (identity state field)
- `crates/bot/src/telegram/dashboard/identity.rs`
- `crates/right-dashboard/src/identity_files.rs` / `api_types.rs` (state field)
- `crates/right-db/src/migrations/` registry
- `ARCHITECTURE.md`
