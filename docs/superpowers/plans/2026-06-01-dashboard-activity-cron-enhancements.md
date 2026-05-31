# Activity Cron View Enhancements — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enrich the existing Activity-tab cron view in place — human-readable schedule + next-fire time, per-cron actual spend (24h / 7d) with a sort control, a fix for run-detail collapse, and a delete-cron button with confirmation.

**Architecture:** Backend read-model (`right-dashboard`) gains a small self-contained schedule-presentation helper (built on the `cron` + `cron-descriptor` workspace crates, no `right-agent` production dependency) and a per-cron spend rollup; `CronCard` gains four fields. The delete route lives in the bot crate (`crates/bot/src/telegram/dashboard.rs`), which already depends on `right-agent`, and delegates to the bot-owned `right_agent::cron_spec::delete_spec` — satisfying the dashboard write contract. The frontend adds a pure sort helper, a Telegram-native confirm/alert helper, a container-level collapse toggle, and view changes.

**Tech Stack:** Rust (edition 2024, axum, `cron` 0.16, `cron-descriptor` 0.1, `right-db`/turso), Vue 3 + TypeScript, Vitest + `@vue/server-renderer`.

**Conventions:**
- Work in place on the current branch (`master`); do **not** create a new branch.
- Rust commands run under devenv: `devenv shell -- cargo ...`.
- Frontend commands run in `crates/right-dashboard/frontend`: `npm run test`.
- TDD: write the failing test first, watch it fail, implement, watch it pass, commit.
- No ARCHITECTURE.md / PROMPT_SYSTEM.md changes: no new invariant, tool, or prompt surface. The delete route complies with the already-documented dashboard write contract.
- Final full workspace test (`devenv shell -- cargo test --workspace`) is mandatory at the end (Task 10), in addition to the targeted tests per task.

---

## File Structure

**Backend (Rust):**
- Create `crates/right-dashboard/src/read_model/schedule.rs` — pure schedule presentation (`describe`, `next_run_at`) + unit tests.
- Modify `crates/right-dashboard/src/read_model.rs` — declare `mod schedule`.
- Modify `crates/right-dashboard/Cargo.toml` — add `cron`, `cron-descriptor` workspace deps.
- Modify `crates/right-dashboard/src/api_types.rs` — extend `CronCard`.
- Modify `crates/right-dashboard/src/read_model/activity.rs` — spend rollup + populate new fields + tests.
- Modify `crates/bot/src/telegram/dashboard.rs` — delete route, handler, test.

**Frontend (TypeScript / Vue):**
- Modify `crates/right-dashboard/frontend/src/types.ts` — extend `CronCard`.
- Create `crates/right-dashboard/frontend/src/views/cronSort.ts` (+ `cronSort.test.ts`).
- Modify `crates/right-dashboard/frontend/src/telegram.ts` (+ `telegram.test.ts`) — `confirmAction`, `alertMessage`.
- Modify `crates/right-dashboard/frontend/src/api.ts` — `deleteCron`.
- Modify `crates/right-dashboard/frontend/src/views/ActivityContainer.vue` — collapse toggle + delete handler.
- Modify `crates/right-dashboard/frontend/src/views/ActivityView.vue` (+ `ActivityView.test.ts`) — schedule, spend grid, sort control, delete button.

---

## Task 1: Backend schedule-presentation helper

**Files:**
- Modify: `crates/right-dashboard/Cargo.toml`
- Create: `crates/right-dashboard/src/read_model/schedule.rs`
- Modify: `crates/right-dashboard/src/read_model.rs:8-19` (module declarations)

- [ ] **Step 1: Add the workspace deps**

In `crates/right-dashboard/Cargo.toml`, under `[dependencies]` (after the `chrono` line), add:

```toml
cron = { workspace = true }
cron-descriptor = { workspace = true }
```

- [ ] **Step 2: Create the helper with failing tests**

Create `crates/right-dashboard/src/read_model/schedule.rs`:

```rust
//! Pure, DB-free presentation helpers for cron schedules. Kept local to the
//! dashboard read-model so the presentation crate does not take a production
//! dependency on the heavy `right-agent` crate (which owns the canonical cron
//! semantics). The 5-field→7-field conversion mirrors
//! `crates/bot/src/cron.rs::to_7field` exactly so the next-fire time shown in
//! the dashboard matches what the reconciler actually computes.
//!
//! A malformed schedule never errors the overview: `describe` falls back to
//! the raw string and `next_run_at` returns `None`. Schedules are validated at
//! creation time; here we only render.

use std::str::FromStr;

use chrono::{DateTime, Utc};

/// Human-readable schedule label.
/// - one-shot absolute (`run_at` present) → `Once at <run_at>`
/// - `@immediate` → `Immediately (next tick)`
/// - cron expression → `cron_descriptor` text, falling back to the raw string
pub(crate) fn describe(schedule: &str, run_at: Option<&str>) -> String {
    if let Some(run_at) = run_at {
        return format!("Once at {run_at}");
    }
    if schedule == "@immediate" {
        return "Immediately (next tick)".to_string();
    }
    match cron_descriptor::cronparser::cron_expression_descriptor::get_description_cron(schedule) {
        Ok(desc) => desc,
        Err(_) => schedule.to_string(),
    }
}

/// Next fire time from `now`.
/// - `run_at` present → that instant (absolute one-shot)
/// - `@immediate` → `None` (fires on the next reconcile tick; the label carries
///   the meaning)
/// - cron expression → `cron::Schedule::after(now).next()`
/// - unparseable / no future fire → `None`
pub(crate) fn next_run_at(
    schedule: &str,
    run_at: Option<&str>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if let Some(run_at) = run_at {
        return DateTime::parse_from_rfc3339(run_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    if schedule == "@immediate" {
        return None;
    }
    let seven_field = format!("0 {} *", schedule.trim());
    let parsed = cron::Schedule::from_str(&seven_field).ok()?;
    parsed.after(&now).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-06-01T09:00:00Z".parse().unwrap()
    }

    #[test]
    fn describe_returns_human_text_for_valid_cron() {
        let desc = describe("0 8 * * *", None);
        assert_ne!(desc, "0 8 * * *");
        assert!(!desc.is_empty());
    }

    #[test]
    fn describe_falls_back_to_raw_for_unparseable() {
        assert_eq!(describe("not-a-cron", None), "not-a-cron");
    }

    #[test]
    fn describe_handles_run_at_and_immediate() {
        assert_eq!(
            describe("ignored", Some("2026-06-02T10:00:00Z")),
            "Once at 2026-06-02T10:00:00Z"
        );
        assert_eq!(describe("@immediate", None), "Immediately (next tick)");
    }

    #[test]
    fn next_run_at_computes_next_cron_fire() {
        // 08:00 daily, now is 09:00 on 2026-06-01 → next fire is 2026-06-02T08:00:00Z.
        let next = next_run_at("0 8 * * *", None, now()).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-06-02T08:00:00+00:00");
    }

    #[test]
    fn next_run_at_uses_run_at_when_present() {
        let next = next_run_at("ignored", Some("2026-06-02T10:00:00Z"), now()).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-06-02T10:00:00+00:00");
    }

    #[test]
    fn next_run_at_is_none_for_immediate_and_unparseable() {
        assert!(next_run_at("@immediate", None, now()).is_none());
        assert!(next_run_at("not-a-cron", None, now()).is_none());
    }
}
```

- [ ] **Step 3: Declare the module**

In `crates/right-dashboard/src/read_model.rs`, in the `#[path = ...]` module block (around lines 8-19), add (keep alphabetical-ish ordering, place after `run_summary`):

```rust
#[path = "read_model/schedule.rs"]
mod schedule;
```

- [ ] **Step 4: Run the tests — expect them to pass once deps resolve**

Run: `devenv shell -- cargo test -p right-dashboard schedule::`
Expected: 6 tests pass. If `cron`/`cron-descriptor` fail to resolve, confirm both exist under `[workspace.dependencies]` in the root `Cargo.toml` (`cron = "0.16"`, `cron-descriptor = "0.1"`).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/Cargo.toml crates/right-dashboard/src/read_model/schedule.rs crates/right-dashboard/src/read_model.rs
git commit -m "feat(dashboard): cron schedule presentation helper (human text + next fire)"
```

---

## Task 2: Extend CronCard with schedule_human, next_run_at, spend windows

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs:242-252` (`CronCard`)
- Modify: `crates/right-dashboard/src/read_model/activity.rs` (imports, spend query, build loop, tests)

- [ ] **Step 1: Extend the `CronCard` struct**

In `crates/right-dashboard/src/api_types.rs`, change `CronCard` (lines 242-252) to add four fields after `max_budget_usd`:

```rust
pub struct CronCard {
    pub job_name: String,
    pub schedule: String,
    pub schedule_human: String,
    pub recurring: bool,
    pub run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub max_budget_usd: f64,
    pub spend_24h_usd: f64,
    pub spend_7d_usd: f64,
    pub last_run: Option<RunSummary>,
    pub recent_runs: Vec<RunSummary>,
}
```

- [ ] **Step 2: Write the failing test for the new fields**

In `crates/right-dashboard/src/read_model/activity.rs`, add this test inside the existing `mod tests` (the `fixture()` already inserts a `daily` cron with a `usage_events` row of `0.25` at `2026-05-20T08:01:00Z`):

```rust
#[tokio::test]
async fn activity_overview_populates_schedule_human_next_run_and_spend() {
    let (_dir, conn) = fixture().await;
    // Second usage event for the same job, ~8 days earlier — counts toward 7d
    // only relative to a generated_at far enough ahead. Use generated_at one
    // hour after the fixture run so both 24h and 7d windows include 0.25.
    let response = activity_overview(
        &conn,
        ActivityOverviewInput {
            agent: "agent-a".to_owned(),
            generated_at: "2026-05-20T09:00:00Z".to_owned(),
            refresh_interval_secs: 30,
            foreground: vec![],
        },
    )
    .await
    .unwrap();

    let card = &response.crons[0];
    // schedule is "0 8 * * *" in the fixture → human text differs from raw.
    assert_ne!(card.schedule_human, card.schedule);
    assert!(!card.schedule_human.is_empty());
    // recurring daily cron → next fire after 2026-05-20T09:00:00Z is 05-21T08:00.
    assert_eq!(
        card.next_run_at.as_deref(),
        Some("2026-05-21T08:00:00+00:00")
    );
    // 0.25 spent at 08:01, inside both windows.
    assert!((card.spend_24h_usd - 0.25).abs() < 1e-9);
    assert!((card.spend_7d_usd - 0.25).abs() < 1e-9);
}

#[tokio::test]
async fn activity_overview_spend_excludes_events_outside_windows() {
    let (_dir, conn) = fixture().await;
    // generated_at 10 days after the fixture event → outside 7d window entirely.
    let response = activity_overview(
        &conn,
        ActivityOverviewInput {
            agent: "agent-a".to_owned(),
            generated_at: "2026-05-30T09:00:00Z".to_owned(),
            refresh_interval_secs: 30,
            foreground: vec![],
        },
    )
    .await
    .unwrap();
    let card = &response.crons[0];
    assert_eq!(card.spend_24h_usd, 0.0);
    assert_eq!(card.spend_7d_usd, 0.0);
}
```

- [ ] **Step 2b: Run to verify failure**

Run: `devenv shell -- cargo test -p right-dashboard activity_overview_populates_schedule_human_next_run_and_spend`
Expected: FAIL to compile (`CronCard` has new fields not yet set in `activity.rs`, and `schedule`/spend not computed).

- [ ] **Step 3: Add imports and the spend rollup query**

In `crates/right-dashboard/src/read_model/activity.rs`, add to the top imports:

```rust
use std::collections::HashMap;
```

Add this function near `today_cost_usd` (it sums per-job cron spend over the 7-day window, conditionally splitting out the 24h sub-window in one pass):

```rust
/// Per-job cron spend for the last 24h and last 7d, keyed by `job_name`.
/// One grouped pass over `usage_events`; jobs with no events are simply absent
/// from the map and default to `0.0` at the call site.
async fn cron_spend_windows(
    conn: &Connection,
    now: &DateTime<Utc>,
) -> Result<HashMap<String, (f64, f64)>, ReadModelError> {
    let since_24h = (*now - Duration::days(1)).to_rfc3339();
    let since_7d = (*now - Duration::days(7)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT job_name,
                COALESCE(SUM(CASE WHEN ts >= ?1 THEN total_cost_usd ELSE 0 END), 0.0),
                COALESCE(SUM(total_cost_usd), 0.0)
         FROM usage_events
         WHERE ts >= ?2 AND job_name IS NOT NULL
         GROUP BY job_name",
    )?;
    let rows = stmt
        .query_map(params![since_24h, since_7d], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })
        .await?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(job, spend_24h, spend_7d)| (job, (spend_24h, spend_7d)))
        .collect())
}
```

- [ ] **Step 4: Populate the new fields in the build loop**

In `activity_overview` (lines ~24-65), compute `now` and the spend map before the loop, and set the new fields. Replace the loop region (lines 49-65) with:

```rust
    let now = parse_utc(&input.generated_at)?;
    let spend = cron_spend_windows(conn, &now).await?;

    let mut crons = Vec::with_capacity(cron_rows.len());
    for (job_name, schedule, recurring, run_at, target_chat_id, target_thread_id, max_budget_usd) in
        cron_rows
    {
        let recent_runs = cron_runs(conn, &job_name).await?;
        let (spend_24h_usd, spend_7d_usd) = spend.get(&job_name).copied().unwrap_or((0.0, 0.0));
        let schedule_human = schedule::describe(&schedule, run_at.as_deref());
        let next_run_at = schedule::next_run_at(&schedule, run_at.as_deref(), now)
            .map(|dt| dt.to_rfc3339());
        crons.push(CronCard {
            job_name,
            schedule,
            schedule_human,
            recurring,
            run_at,
            next_run_at,
            target_chat_id,
            target_thread_id,
            max_budget_usd,
            spend_24h_usd,
            spend_7d_usd,
            last_run: recent_runs.first().cloned(),
            recent_runs,
        });
    }
```

Add `schedule` to the module's `use super::{...}` if needed — it is a sibling module, referenced as `schedule::describe`; since `mod schedule` is declared in `read_model.rs`, reference it via `super::schedule`. Add to the existing `use super::{ReadModelError, parse_utc};` line:

```rust
use super::{ReadModelError, parse_utc, schedule};
```

- [ ] **Step 5: Run the tests — expect pass**

Run: `devenv shell -- cargo test -p right-dashboard activity_overview`
Expected: all `activity_overview*` tests pass, including the two new ones and the pre-existing `activity_overview_builds_cron_cards_and_active_background`.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model/activity.rs
git commit -m "feat(dashboard): cron cards carry human schedule, next fire, and 24h/7d spend"
```

---

## Task 3: Delete-cron route + handler (bot)

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs` (route registration ~line 96, new handler, new test)

- [ ] **Step 1: Write the failing route test**

In `crates/bot/src/telegram/dashboard.rs`, inside the `mod tests`, add a DELETE helper and a test. Place near the other request helpers:

```rust
    async fn delete(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
    ) -> StatusCode {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder().uri(path).method("DELETE");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        router
            .oneshot(builder.body(Body::empty()).expect("valid request"))
            .await
            .expect("router response")
            .status()
    }

    #[tokio::test]
    async fn delete_cron_removes_spec_and_is_idempotent_404() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true)
            .await
            .expect("open db");
        conn.execute(
            "INSERT INTO cron_specs (job_name, schedule, prompt, max_budget_usd, created_at, updated_at, recurring)
             VALUES ('daily', '0 8 * * *', 'p', 1.0, '2026-05-20T00:00:00Z', '2026-05-20T00:00:00Z', 1)",
            [],
        )
        .await
        .expect("insert cron spec");
        drop(conn);

        // Unauthenticated → rejected.
        assert_eq!(
            delete("/dashboard/alpha/api/v1/crons/daily", None, dir.path().to_path_buf()).await,
            StatusCode::UNAUTHORIZED
        );

        // Authenticated delete → 200.
        let auth = signed_init_data(42);
        assert_eq!(
            delete(
                "/dashboard/alpha/api/v1/crons/daily",
                Some(auth.clone()),
                dir.path().to_path_buf()
            )
            .await,
            StatusCode::OK
        );

        // Row is gone.
        let conn = right_db::open_connection(dir.path(), false)
            .await
            .expect("reopen db");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM cron_specs WHERE job_name = 'daily'", [], |row| row.get(0))
            .await
            .expect("count");
        assert_eq!(count, 0);
        drop(conn);

        // Second delete → 404.
        assert_eq!(
            delete("/dashboard/alpha/api/v1/crons/daily", Some(auth), dir.path().to_path_buf()).await,
            StatusCode::NOT_FOUND
        );
    }
```

> Note on the unauthenticated status: `authenticate_api` returns 401 for a missing/invalid `tma` token (see `DashboardApiError.isLocked` covering 401/403). If the existing auth path returns 403 instead, adjust the assertion to `StatusCode::FORBIDDEN` to match `authenticate_api`'s actual rejection code — check the first auth-rejection assertion already present in this test module and mirror it.

- [ ] **Step 2: Run to verify failure**

Run: `devenv shell -- cargo test -p right-bot delete_cron_removes_spec_and_is_idempotent_404`
Expected: FAIL — route not registered (404 for the authenticated delete, or compile error if `delete` helper name clashes; if it clashes with the imported axum `delete`, rename the test helper to `delete_req`).

- [ ] **Step 3: Register the route**

In `build_dashboard_router`, after the activity run-detail route (around line 96-98), add:

```rust
        .route(
            "/dashboard/{agent}/api/v1/crons/{job_name}",
            delete(handle_delete_cron),
        )
```

(`delete` is already imported at the top of the file — it is used by the mcp/providers DELETE routes.)

- [ ] **Step 4: Add the handler**

Add near `handle_pin_skill`:

```rust
async fn handle_delete_cron(
    AxumPath((agent, job_name)): AxumPath<(String, String)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let conn = match right_db::open_connection(&state.agent_dir, false).await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::error!(agent = %state.agent_name, job = %job_name, "dashboard cron delete: open db failed: {error:#}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_open_failed",
                Some("failed to open database"),
            );
        }
    };

    match right_agent::cron_spec::delete_spec(&conn, &job_name, &state.agent_dir).await {
        Ok(_) => Json(serde_json::json!({ "deleted": true, "job_name": job_name })).into_response(),
        Err(error) if error.contains("not found") => {
            json_error(StatusCode::NOT_FOUND, "not_found", Some("cron job not found"))
        }
        Err(error) => {
            tracing::error!(agent = %state.agent_name, job = %job_name, "dashboard cron delete failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cron_delete_failed",
                Some("failed to delete cron job"),
            )
        }
    }
}
```

> `right_agent::cron_spec::delete_spec(&Connection, &str, &Path)` deletes the `cron_specs` row and removes the lock file; it returns `Err("job '<name>' not found")` when the row is absent (matched by `contains("not found")`). `async_runs` history is intentionally left intact. This is the same function the agent's `cron_delete` MCP tool calls, so reconciler teardown is already handled.

- [ ] **Step 5: Run to verify pass**

Run: `devenv shell -- cargo test -p right-bot delete_cron_removes_spec_and_is_idempotent_404`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/dashboard.rs
git commit -m "feat(dashboard): DELETE /crons/{job_name} route via delete_spec"
```

---

## Task 4: Frontend CronCard type fields

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts:285-295` (`CronCard`)

- [ ] **Step 1: Mirror the backend fields**

Change `CronCard` (lines 285-295) to:

```typescript
export interface CronCard {
  job_name: string
  schedule: string
  schedule_human: string
  recurring: boolean
  run_at: string | null
  next_run_at: string | null
  target_chat_id: number | null
  target_thread_id: number | null
  max_budget_usd: number
  spend_24h_usd: number
  spend_7d_usd: number
  last_run: RunSummary | null
  recent_runs: RunSummary[]
}
```

- [ ] **Step 2: Type-check**

Run: `cd crates/right-dashboard/frontend && npm run build` (or `npx vue-tsc --noEmit` if available)
Expected: type errors ONLY in `ActivityView.vue`/tests that have not yet been updated are acceptable at this stage; the `types.ts` file itself compiles. If the project has no standalone typecheck, defer verification to Task 9.

- [ ] **Step 3: Commit**

```bash
git add crates/right-dashboard/frontend/src/types.ts
git commit -m "feat(dashboard): CronCard TS type gains schedule_human, next_run_at, spend"
```

---

## Task 5: Client-side cron sort helper

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/cronSort.ts`
- Create: `crates/right-dashboard/frontend/src/views/cronSort.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/cronSort.test.ts`:

```typescript
import { describe, expect, it } from 'vitest'

import type { CronCard } from '../types'
import { sortCrons } from './cronSort'

function card(name: string, s24: number, s7: number): CronCard {
  return {
    job_name: name,
    schedule: '0 8 * * *',
    schedule_human: 'daily',
    recurring: true,
    run_at: null,
    next_run_at: null,
    target_chat_id: null,
    target_thread_id: null,
    max_budget_usd: 1,
    spend_24h_usd: s24,
    spend_7d_usd: s7,
    last_run: null,
    recent_runs: [],
  }
}

describe('sortCrons', () => {
  const crons = [card('beta', 1, 5), card('alpha', 3, 2), card('gamma', 2, 9)]

  it('sorts by name ascending by default', () => {
    expect(sortCrons(crons, 'name').map((c) => c.job_name)).toEqual(['alpha', 'beta', 'gamma'])
  })

  it('sorts by 24h spend descending, name tie-break', () => {
    expect(sortCrons(crons, 'spend_24h').map((c) => c.job_name)).toEqual(['alpha', 'gamma', 'beta'])
  })

  it('sorts by 7d spend descending', () => {
    expect(sortCrons(crons, 'spend_7d').map((c) => c.job_name)).toEqual(['gamma', 'beta', 'alpha'])
  })

  it('does not mutate the input array', () => {
    const input = [card('b', 0, 0), card('a', 0, 0)]
    sortCrons(input, 'name')
    expect(input.map((c) => c.job_name)).toEqual(['b', 'a'])
  })
})
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/right-dashboard/frontend && npm run test -- cronSort`
Expected: FAIL — `./cronSort` not found.

- [ ] **Step 3: Implement**

Create `crates/right-dashboard/frontend/src/views/cronSort.ts`:

```typescript
import type { CronCard } from '../types'

export type CronSortMode = 'name' | 'spend_24h' | 'spend_7d'

export const CRON_SORT_MODES: { value: CronSortMode; label: string }[] = [
  { value: 'name', label: 'Name' },
  { value: 'spend_24h', label: 'Spend 24h' },
  { value: 'spend_7d', label: 'Spend 7d' },
]

export function sortCrons(crons: CronCard[], mode: CronSortMode): CronCard[] {
  const copy = [...crons]
  const byName = (a: CronCard, b: CronCard) => a.job_name.localeCompare(b.job_name)
  switch (mode) {
    case 'spend_24h':
      return copy.sort((a, b) => b.spend_24h_usd - a.spend_24h_usd || byName(a, b))
    case 'spend_7d':
      return copy.sort((a, b) => b.spend_7d_usd - a.spend_7d_usd || byName(a, b))
    default:
      return copy.sort(byName)
  }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/right-dashboard/frontend && npm run test -- cronSort`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/cronSort.ts crates/right-dashboard/frontend/src/views/cronSort.test.ts
git commit -m "feat(dashboard): pure cron sort helper (name / 24h / 7d spend)"
```

---

## Task 6: Telegram confirm + alert helpers

**Files:**
- Modify: `crates/right-dashboard/frontend/src/telegram.ts` (interface + two functions)
- Modify: `crates/right-dashboard/frontend/src/telegram.test.ts` (tests)

- [ ] **Step 1: Write the failing tests**

Append to `crates/right-dashboard/frontend/src/telegram.test.ts`:

```typescript
describe('confirmAction', () => {
  it('resolves true/false from WebApp.showConfirm', async () => {
    const showConfirm = vi.fn((_msg: string, cb: (ok: boolean) => void) => cb(true))
    const webApp = { showConfirm } as unknown as TelegramWebApp
    await expect(confirmAction('Delete?', webApp)).resolves.toBe(true)
    expect(showConfirm).toHaveBeenCalledWith('Delete?', expect.any(Function))
  })

  it('falls back to false when no WebApp and no window.confirm', async () => {
    await expect(confirmAction('Delete?', undefined, undefined)).resolves.toBe(false)
  })
})

describe('alertMessage', () => {
  it('uses WebApp.showAlert when present', async () => {
    const showAlert = vi.fn((_msg: string, cb?: () => void) => cb?.())
    const webApp = { showAlert } as unknown as TelegramWebApp
    await alertMessage('Boom', webApp)
    expect(showAlert).toHaveBeenCalledWith('Boom', expect.any(Function))
  })
})
```

Add `confirmAction`, `alertMessage` to the existing import block at the top of the test file (from `'./telegram'`).

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/right-dashboard/frontend && npm run test -- telegram`
Expected: FAIL — `confirmAction`/`alertMessage` not exported.

- [ ] **Step 3: Implement**

In `crates/right-dashboard/frontend/src/telegram.ts`, extend the `TelegramWebApp` interface (after `openLink`):

```typescript
  showConfirm?: (message: string, callback: (confirmed: boolean) => void) => void
  showAlert?: (message: string, callback?: () => void) => void
```

Add at the end of the file:

```typescript
/** Native Mini-App confirmation; falls back to `window.confirm`, else `false`. */
export function confirmAction(
  message: string,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  confirmFn: ((message?: string) => boolean) | undefined =
    typeof window === 'undefined' ? undefined : window.confirm.bind(window),
): Promise<boolean> {
  if (typeof webApp?.showConfirm === 'function') {
    return new Promise((resolve) => webApp.showConfirm!(message, resolve))
  }
  return Promise.resolve(confirmFn ? confirmFn(message) : false)
}

/** Native Mini-App alert; falls back to `window.alert`, else no-op. */
export function alertMessage(
  message: string,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  alertFn: ((message?: string) => void) | undefined =
    typeof window === 'undefined' ? undefined : window.alert.bind(window),
): Promise<void> {
  if (typeof webApp?.showAlert === 'function') {
    return new Promise((resolve) => webApp.showAlert!(message, resolve))
  }
  alertFn?.(message)
  return Promise.resolve()
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cd crates/right-dashboard/frontend && npm run test -- telegram`
Expected: all telegram tests pass (existing + 3 new).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/telegram.ts crates/right-dashboard/frontend/src/telegram.test.ts
git commit -m "feat(dashboard): Telegram-native confirmAction/alertMessage helpers"
```

---

## Task 7: deleteCron API client

**Files:**
- Modify: `crates/right-dashboard/frontend/src/api.ts` (new export, after `setSkillPinned`)

- [ ] **Step 1: Add the client function**

In `crates/right-dashboard/frontend/src/api.ts`, after the `setSkillPinned` function, add:

```typescript
export function deleteCron(jobName: string): Promise<{ deleted: boolean; job_name: string }> {
  return requestJson<{ deleted: boolean; job_name: string }>(
    `api/v1/crons/${encodeURIComponent(jobName)}`,
    { method: 'DELETE' },
  )
}
```

- [ ] **Step 2: Type-check (lightweight)**

Run: `cd crates/right-dashboard/frontend && npm run test -- api` (if an api test exists) or defer to Task 9's full `npm run test`.
Expected: no new errors introduced by this file.

- [ ] **Step 3: Commit**

```bash
git add crates/right-dashboard/frontend/src/api.ts
git commit -m "feat(dashboard): deleteCron API client (DELETE /crons/{job_name})"
```

---

## Task 8: Collapse toggle + delete handler in ActivityContainer

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ActivityContainer.vue`
- Create: `crates/right-dashboard/frontend/src/views/activitySelection.test.ts` additions OR a small `runToggle.ts` helper

The collapse fix is a one-line predicate; extract it so it is unit-testable rather than buried in the async `selectRun`.

- [ ] **Step 1: Write the failing helper test**

Append to `crates/right-dashboard/frontend/src/views/activitySelection.test.ts`:

```typescript
import { isSameRunSelected } from './activitySelection'

describe('isSameRunSelected', () => {
  it('is true only when the clicked run equals the currently selected run', () => {
    expect(isSameRunSelected('r1', 'r1')).toBe(true)
    expect(isSameRunSelected('r1', 'r2')).toBe(false)
    expect(isSameRunSelected(null, 'r1')).toBe(false)
  })
})
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/right-dashboard/frontend && npm run test -- activitySelection`
Expected: FAIL — `isSameRunSelected` not exported.

- [ ] **Step 3: Add the helper**

In `crates/right-dashboard/frontend/src/views/activitySelection.ts`, add:

```typescript
export function isSameRunSelected(selectedRunId: string | null, runId: string): boolean {
  return selectedRunId === runId
}
```

- [ ] **Step 4: Wire the toggle and delete handler into the container**

In `crates/right-dashboard/frontend/src/views/ActivityContainer.vue`:

Update the imports:

```typescript
import { DashboardApiError, overview as activityOverview, runDetail, deleteCron } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import { activityContainsRun, isSameRunSelected } from './activitySelection'
import { alertMessage, confirmAction } from '../telegram'
import ActivityView from './ActivityView.vue'
import type { CronCard, RunDetailResponse, RunSummary } from '../types'
```

At the start of `selectRun`, add the toggle (collapse when the same run is clicked again):

```typescript
async function selectRun(run: RunSummary): Promise<void> {
  const runId = run.id
  if (isSameRunSelected(selectedRunId.value, runId)) {
    selectedRunId.value = null
    selectedRun.value = null
    detailError.value = null
    return
  }
  selectedRunId.value = runId
  // ...existing body unchanged...
```

Add the delete handler:

```typescript
async function onDeleteCron(cron: CronCard): Promise<void> {
  const confirmed = await confirmAction(`Delete cron "${cron.job_name}"? This cannot be undone.`)
  if (!confirmed) {
    return
  }
  try {
    await deleteCron(cron.job_name)
    await refresh()
  } catch (err) {
    await alertMessage(err instanceof Error ? err.message : 'Failed to delete cron')
  }
}
```

Pass the new handler to the view template:

```vue
  <ActivityView
    :overview="activity"
    :selected-run="selectedRun"
    :selected-run-id="selectedRunId"
    :loading-detail="loadingDetail"
    :detail-error="detailError"
    @select-run="selectRun"
    @delete-cron="onDeleteCron"
  />
```

- [ ] **Step 5: Run to verify pass**

Run: `cd crates/right-dashboard/frontend && npm run test -- activitySelection`
Expected: helper tests pass. (Container behavior is verified via the view test in Task 9.)

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/activitySelection.ts crates/right-dashboard/frontend/src/views/activitySelection.test.ts crates/right-dashboard/frontend/src/views/ActivityContainer.vue
git commit -m "fix(dashboard): toggle run-detail collapse; wire cron delete handler"
```

---

## Task 9: ActivityView — schedule, spend grid, sort control, delete button

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.test.ts`

- [ ] **Step 1: Write failing view tests**

Append to `crates/right-dashboard/frontend/src/views/ActivityView.test.ts` (the file already has `renderToString`/`createSSRApp`/`h` imports and a `render` helper; reuse them). Add a cron-card fixture and tests:

```typescript
function cronCard(overrides: Record<string, unknown> = {}) {
  return {
    job_name: 'daily', schedule: '0 8 * * *', schedule_human: 'At 08:00, every day',
    recurring: true, run_at: null, next_run_at: '2026-06-02T08:00:00Z',
    target_chat_id: 123, target_thread_id: null, max_budget_usd: 2,
    spend_24h_usd: 0.25, spend_7d_usd: 1.5, last_run: null, recent_runs: [], ...overrides,
  }
}

function overviewWithCron(cron: Record<string, unknown>) {
  return {
    agent: 'a', generated_at: '2026-06-01T12:00:00Z', refresh_interval_secs: 5,
    summary: { cron_count: 1, active_cron_count: 0, failed_recent_cron_count: 0, today_cost_usd: 0 },
    crons: [cron], failed_runs: [], active: { foreground: [], background: [] },
  }
}

describe('ActivityView cron card', () => {
  it('renders human schedule and spend figures, not the raw cron expression as the primary line', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toContain('At 08:00, every day')
    expect(html).toContain('$0.25') // 24h spend
    expect(html).toContain('$1.50') // 7d spend
  })

  it('renders a delete button for the cron', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toMatch(/cron-delete/)
  })

  it('renders the sort control', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toContain('Spend 24h')
    expect(html).toContain('Spend 7d')
  })
})
```

- [ ] **Step 2: Run to verify failure**

Run: `cd crates/right-dashboard/frontend && npm run test -- ActivityView`
Expected: FAIL — human schedule/spend/sort/delete markup absent.

- [ ] **Step 3: Update the script block**

In `crates/right-dashboard/frontend/src/views/ActivityView.vue` `<script setup>`:

`computed`, `ref` (line 2), `money`, `shortDate` (line 10), and `CronCard` (line 11) are **already imported** — do not re-add them. Add only the cronSort import (after the existing imports):

```typescript
import { sortCrons, CRON_SORT_MODES, type CronSortMode } from './cronSort'
```

Add the sort state + sorted list (after the existing `failuresOpen`/`failures` refs):

```typescript
const sortMode = ref<CronSortMode>('name')
const sortedCrons = computed(() => sortCrons(props.overview?.crons ?? [], sortMode.value))
```

Extend `defineEmits` to add the delete event:

```typescript
const emit = defineEmits<{
  selectRun: [run: RunSummary]
  deleteCron: [cron: CronCard]
}>()
```

- [ ] **Step 4: Update the template — header, sort control, grid, delete button**

Replace the cron `<section class="list-stack">` opening and the per-card header + meta-grid (lines ~58-88) with:

```vue
    <section class="list-stack">
      <div class="cron-sort">
        <label>Sort
          <select v-model="sortMode" class="cron-sort-select">
            <option v-for="opt in CRON_SORT_MODES" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
          </select>
        </label>
      </div>

      <article v-if="(overview?.crons.length ?? 0) === 0" class="empty-panel">No cron jobs</article>

      <article v-for="cron in sortedCrons" :key="cron.job_name" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">{{ cron.recurring ? 'Recurring' : 'One shot' }}</p>
            <h2>{{ cron.job_name }}</h2>
            <p class="muted-line" :title="cron.schedule">{{ cron.schedule_human }}</p>
            <p v-if="cron.target_chat_id" class="muted-line">
              → {{ cron.target_chat_id }}<span v-if="cron.target_thread_id">/{{ cron.target_thread_id }}</span>
            </p>
          </div>
          <div class="panel-head-actions">
            <StatusPill :status="cronStatus(cron)" />
            <button class="cron-delete" type="button" @click="emit('deleteCron', cron)">Delete</button>
          </div>
        </header>

        <dl class="meta-grid">
          <div>
            <dt>Next</dt>
            <dd>{{ shortDate(cron.next_run_at) }}</dd>
          </div>
          <div>
            <dt>24h</dt>
            <dd>{{ money(cron.spend_24h_usd) }}</dd>
          </div>
          <div>
            <dt>7d</dt>
            <dd>{{ money(cron.spend_7d_usd) }}</dd>
          </div>
          <div>
            <dt>Cap</dt>
            <dd>{{ money(cron.max_budget_usd) }}</dd>
          </div>
        </dl>
```

Leave the `<div class="row-list">` runs section and the rest of the card unchanged. Note: the `v-for` source changes from `overview?.crons ?? []` to `sortedCrons`.

- [ ] **Step 5: Add minimal styles**

In the `<style>` block of `ActivityView.vue` (or the shared style if scoped styles are not used here — match the file's existing approach), add:

```css
.panel-head-actions { display: flex; align-items: center; gap: 0.5rem; }
.cron-delete { font-size: 0.75rem; padding: 0.2rem 0.5rem; border-radius: 0.4rem; border: 1px solid var(--danger, #c0392b); color: var(--danger, #c0392b); background: transparent; cursor: pointer; }
.cron-sort { display: flex; justify-content: flex-end; margin-bottom: 0.5rem; }
.cron-sort-select { margin-left: 0.4rem; }
```

> If `ActivityView.vue` has no `<style>` block, add the rules to the global stylesheet that the other `.data-row`/`.meta-grid` classes live in (`App.vue` style globals). Match the surrounding pattern rather than introducing scoped styles unilaterally.

- [ ] **Step 6: Run to verify pass**

Run: `cd crates/right-dashboard/frontend && npm run test -- ActivityView`
Expected: new cron-card tests pass; the pre-existing failures-card tests still pass.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/ActivityView.vue crates/right-dashboard/frontend/src/views/ActivityView.test.ts
git commit -m "feat(dashboard): cron card shows human schedule, spend, sort, and delete"
```

---

## Task 10: Full verification

**Files:** none (verification + fixups only)

- [ ] **Step 1: Frontend — full test + build**

Run: `cd crates/right-dashboard/frontend && npm run test && npm run build`
Expected: all Vitest suites pass; production build succeeds (this is the real TypeScript typecheck — fix any `CronCard` field mismatches surfaced here).

- [ ] **Step 2: Rust — clippy on the two touched crates**

Run: `devenv shell -- cargo clippy -p right-dashboard -p right-bot --all-targets`
Expected: no warnings. Common fixups: unused `schedule` import path, `HashMap` import.

- [ ] **Step 3: Rust — full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing unrelated failures separately; do not let the new code introduce failures.

- [ ] **Step 4: Rust — debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: clean debug build.

- [ ] **Step 5: Manual smoke (optional but recommended)**

If a dev bot is running, open the dashboard `/activity`, confirm: human schedule text, next-fire on a recurring cron, 24h/7d spend, sort selector reorders, clicking a run twice collapses it, and Delete prompts a confirm then removes the cron after refresh.

- [ ] **Step 6: Final commit (if any fixups)**

```bash
git add -A
git commit -m "chore(dashboard): cron view enhancements — clippy/typecheck fixups"
```

---

## Self-Review Notes (coverage against the spec)

- Human-readable schedule → Task 1 (`describe`) + Task 9 (render).
- Next-fire for recurring → Task 1 (`next_run_at`) + Task 2 (field) + Task 9 (render).
- Actual spend 24h/7d → Task 2 (`cron_spend_windows`) + Task 9 (render).
- Sort by spend → Task 5 (`sortCrons`) + Task 9 (control).
- Collapse fix → Task 8 (`isSameRunSelected` + toggle).
- Delete with confirm → Task 3 (route) + Task 6 (`confirmAction`) + Task 7 (`deleteCron`) + Task 8 (handler) + Task 9 (button).
- Write-contract compliance: delete handler delegates to `right_agent::cron_spec::delete_spec`; no hand-rolled SQL, no agent-file edits beyond the lock file that `delete_spec` owns.
- Run history preserved on delete (spec non-goal honored).
- No prompt-in-card, no new tab, no create/edit (YAGNI / non-goals honored).
