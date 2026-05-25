# Legacy Learning Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the deprecated Stage 2 learning system, including historical learning tables/data, while keeping old learning config keys accepted as warning-only legacy input.

**Architecture:** Add a destructive schema migration that drops deprecated learning tables, then remove all runtime, dashboard, and frontend dependencies on those tables. The surviving learning system is the current prefilter/probe-writer/curator loop backed by `skill_learning_events`, `skill_lifecycle`, `curator_state`, and usage rows.

**Tech Stack:** Rust 2024, local `right-db` libSQL wrapper, Axum dashboard routes, Vue/Vite dashboard frontend, `devenv shell -- cargo`, `devenv shell -- npm`.

---

## Execution Notes

- The approved design is `docs/superpowers/specs/2026-05-25-legacy-learning-cleanup-design.md`.
- Before writing Rust in an execution session, load `rust-dev:rust-dev` if it is available. It was not installed when this plan was written.
- Work in an isolated worktree if executing via subagents.
- Use TDD for each behavior/schema slice.
- Do not remove unrelated compatibility code for allowlists, OpenShell policies, restore path inference, or FTS5 scrubbing.
- Final verification is `devenv shell -- cargo test --workspace`.

## File Map

- Modify: `crates/right-db/src/migrations.rs` — register migration v35 and add migration regression tests.
- Create: `crates/right-db/src/sql/v35_legacy_learning_cleanup.sql` — destructive table drops.
- Modify: `crates/right-agent/src/learned_skills.rs` — remove nudge/review-gate APIs and stop inserting into `skill_nudge_state`.
- Delete: `crates/right-agent/src/learning_episodes.rs` — deprecated episode domain API.
- Modify: `crates/right-agent/src/lib.rs` — remove `learning_episodes` module export.
- Delete: `crates/bot/src/learning_episode.rs`
- Delete: `crates/bot/src/learning_episode_tests.rs`
- Delete: `crates/bot/src/learning_review.rs`
- Delete: `crates/bot/src/learning_review_tests.rs`
- Delete: `crates/bot/src/execution_events.rs` if the audit confirms no live non-legacy consumer.
- Modify: `crates/bot/src/lib.rs`, `crates/bot/src/background.rs`, `crates/bot/src/async_delivery.rs`, `crates/bot/src/cron.rs`, `crates/bot/src/telegram/handler.rs`, `crates/bot/src/telegram/dispatch.rs`, `crates/bot/src/telegram/worker.rs` — remove seed-capture, drain-scheduler, review-spawn, and legacy reply-signal plumbing.
- Modify: `crates/bot/src/telegram/dashboard.rs` — remove episode/report routes and handlers.
- Modify: `crates/right-dashboard/src/read_model.rs` — remove learning episode module export.
- Delete: `crates/right-dashboard/src/read_model/learning_episodes.rs`
- Modify: `crates/right-dashboard/src/read_model/learning.rs` — remove queries against deprecated tables and rebuild learning overview from current sources.
- Modify: `crates/right-dashboard/src/api_types.rs` — remove episode/report DTOs and deprecated overview fields.
- Modify: `crates/right-dashboard/frontend/src/api.ts`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/App.vue`, `crates/right-dashboard/frontend/src/views/KnowledgeView.vue`, `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue` — remove episode/report API usage and display current learning data only.
- Delete: `crates/right-dashboard/frontend/src/views/learning/EpisodesView.vue`
- Modify: `crates/right-dashboard/static/dashboard/**` — regenerated dashboard assets after frontend build.
- Modify: `ARCHITECTURE.md`, `PROMPT_SYSTEM.md`, `docs/architecture/modules.md`, `docs/architecture/sessions.md`, `docs/architecture/mcp.md` — remove Stage 2 and report-only reviewer references.

## Task 0: Baseline And Audit

**Files:**
- Read: `docs/superpowers/specs/2026-05-25-legacy-learning-cleanup-design.md`
- Read: `crates/bot/src/learning_episode.rs`
- Read: `crates/bot/src/learning_review.rs`
- Read: `crates/right-dashboard/src/read_model/learning.rs`
- Read: `crates/right-dashboard/src/read_model/learning_episodes.rs`

- [ ] **Step 1: Confirm clean starting state**

Run:

```bash
devenv shell -- git status --short
```

Expected: no unrelated unstaged edits. If there are unrelated edits, leave them alone and record them in the implementation notes.

- [ ] **Step 2: Run baseline checks**

Run:

```bash
devenv shell -- cargo test -p right-db migrations
devenv shell -- cargo test -p right-agent learned_skills
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard
```

Expected: PASS, or record any pre-existing failures before editing.

- [ ] **Step 3: Audit `execution_events` ownership**

Run:

```bash
devenv shell -- rg -n "execution_events|insert_execution_event|ExecutionEventKind|NewExecutionEvent" crates
```

Expected: all non-test hits are in deprecated Stage 2 files (`crates/bot/src/execution_events.rs`, `crates/bot/src/learning_episode.rs`, `crates/right-agent/src/learning_episodes.rs`, dashboard read models over legacy data, or callsites that exist only to feed those paths). If a live non-legacy consumer remains, stop and update the design before dropping `execution_events`.

## Task 1: Drop Deprecated Learning Tables

**Files:**
- Create: `crates/right-db/src/sql/v35_legacy_learning_cleanup.sql`
- Modify: `crates/right-db/src/migrations.rs`

- [ ] **Step 1: Write the failing migration test**

Add this test inside `#[cfg(test)] mod tests` in `crates/right-db/src/migrations.rs`:

```rust
// crates/right-db/src/migrations.rs
async fn table_exists(conn: &Connection, name: &str) -> bool {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |r| r.get(0),
        )
        .await
        .unwrap();
    exists == 1
}

#[tokio::test]
async fn legacy_learning_cleanup_drops_deprecated_tables() {
    let mut conn = Connection::open_in_memory().await.unwrap();
    MIGRATIONS.to_version(&mut conn, 34).await.unwrap();

    for table in [
        "learning_episodes",
        "skill_nudge_signals",
        "skill_nudge_state",
        "skill_review_reports",
        "execution_events",
    ] {
        assert!(table_exists(&conn, table).await, "{table} should exist before v35");
    }

    MIGRATIONS.to_latest(&mut conn).await.unwrap();

    for table in [
        "learning_episodes",
        "skill_nudge_signals",
        "skill_nudge_state",
        "skill_review_reports",
        "execution_events",
    ] {
        assert!(!table_exists(&conn, table).await, "{table} should be dropped by v35");
    }
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
devenv shell -- cargo test -p right-db legacy_learning_cleanup_drops_deprecated_tables -- --exact
```

Expected: FAIL because migration v35 does not exist and the deprecated tables still exist after `to_latest`.

- [ ] **Step 3: Add the migration SQL**

Create `crates/right-db/src/sql/v35_legacy_learning_cleanup.sql`:

```sql
-- crates/right-db/src/sql/v35_legacy_learning_cleanup.sql
DROP TABLE IF EXISTS learning_episodes;
DROP TABLE IF EXISTS skill_nudge_signals;
DROP TABLE IF EXISTS skill_nudge_state;
DROP TABLE IF EXISTS skill_review_reports;
DROP TABLE IF EXISTS execution_events;
```

- [ ] **Step 4: Register migration v35**

In `crates/right-db/src/migrations.rs`, add:

```rust
// crates/right-db/src/migrations.rs
const V35_SCHEMA: &str = include_str!("sql/v35_legacy_learning_cleanup.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 35;
```

Append this migration after version 34:

```rust
// crates/right-db/src/migrations.rs
Migration {
    version: 35,
    sql: V35_SCHEMA,
    hook: None,
},
```

- [ ] **Step 5: Verify the migration test passes**

Run:

```bash
devenv shell -- cargo test -p right-db legacy_learning_cleanup_drops_deprecated_tables -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/right-db/src/migrations.rs crates/right-db/src/sql/v35_legacy_learning_cleanup.sql
devenv shell -- git commit -m "fix(db): drop legacy learning tables"
```

## Task 2: Remove Nudge/Review-Gate State From `right-agent`

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`
- Delete: `crates/right-agent/src/learning_episodes.rs`
- Modify: `crates/right-agent/src/lib.rs`

- [ ] **Step 1: Add a failing current-learning regression test**

In `crates/right-agent/src/learned_skills.rs`, keep the test helper `conn()` and add:

```rust
// crates/right-agent/src/learned_skills.rs
#[tokio::test]
async fn insert_learning_event_does_not_require_legacy_nudge_state_table() {
    let (_dir, conn) = conn().await;
    let nudge_state_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_nudge_state'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(nudge_state_exists, 0);

    insert_learning_event(
        &conn,
        &LearningEvent {
            invocation_id: "inv-current".to_owned(),
            agent_name: "right".to_owned(),
            action: LearningAction::Create,
            skill_name: "rightx-current-learning".to_owned(),
            phase: LearningPhase::Finish,
            status: Some(LearningStatus::Created),
            hint_outcome: Some("applied_as_hinted".to_owned()),
            reason: Some("captured reusable workflow".to_owned()),
            message: Some("Created current learning skill".to_owned()),
            summary: Some("Current learning writes only skill_learning_events.".to_owned()),
            event_refs: vec!["msg:1".to_owned()],
        },
    )
    .await
    .unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM skill_learning_events WHERE invocation_id='inv-current'",
            [],
            |r| r.get(0),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
devenv shell -- cargo test -p right-agent insert_learning_event_does_not_require_legacy_nudge_state_table -- --exact
```

Expected: FAIL because `insert_learning_event` still inserts into `skill_nudge_state`.

- [ ] **Step 3: Remove legacy nudge/review APIs**

In `crates/right-agent/src/learned_skills.rs`, remove these items:

```rust
// crates/right-agent/src/learned_skills.rs
NudgeSignalKind
NudgeSignalRecord
ReviewTriggerKind
ReviewStatus
ReviewConfidence
SkillReviewReport
ReviewGateInput
ReviewSkipReason
ReviewGateDecision
select_reply_signal
validate_nudge_signal
validate_learning_signal
validate_skill_issue_signal
ensure_nudge_state
increment_turn_nudge_counters
record_nudge_signal
insert_skill_review_report
review_gate_decision
try_mark_review_started
mark_review_finished
mark_review_finished_in_tx
reset_stale_review_running
record_review_failure
clear_review_running
```

Also remove tests that cover only those items. Keep tests for current `LearningEvent` behavior.

- [ ] **Step 4: Stop writing `skill_nudge_state`**

Replace `insert_learning_event` with:

```rust
// crates/right-agent/src/learned_skills.rs
pub async fn insert_learning_event(
    conn: &Connection,
    event: &LearningEvent,
) -> Result<(), DbError> {
    let event_refs_json = serde_json::to_string(&event.event_refs)
        .map_err(|e| DbError::InvalidParameter(e.to_string()))?;
    conn.execute(
        "INSERT INTO skill_learning_events \
         (invocation_id, agent_name, action, skill_name, phase, status, hint_outcome, reason, message, summary, event_refs_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            event.invocation_id.as_str(),
            event.agent_name.as_str(),
            event.action.as_str(),
            event.skill_name.as_str(),
            event.phase.as_str(),
            event.status.map(LearningStatus::as_str),
            event.hint_outcome.as_deref(),
            event.reason.as_deref(),
            event.message.as_deref(),
            event.summary.as_deref(),
            event_refs_json,
        ],
    )
    .await?;
    Ok(())
}
```

- [ ] **Step 5: Remove the legacy episode domain module**

Delete `crates/right-agent/src/learning_episodes.rs`.

Remove this line from `crates/right-agent/src/lib.rs`:

```rust
// crates/right-agent/src/lib.rs
pub mod learning_episodes;
```

- [ ] **Step 6: Verify current learning domain tests pass**

Run:

```bash
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-agent/src/learned_skills.rs crates/right-agent/src/lib.rs crates/right-agent/src/learning_episodes.rs
devenv shell -- git commit -m "refactor(agent): remove legacy learning review domain"
```

## Task 3: Remove Bot Stage 2 Runtime

**Files:**
- Delete: `crates/bot/src/learning_episode.rs`
- Delete: `crates/bot/src/learning_episode_tests.rs`
- Delete: `crates/bot/src/learning_review.rs`
- Delete: `crates/bot/src/learning_review_tests.rs`
- Delete: `crates/bot/src/execution_events.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/background.rs`
- Modify: `crates/bot/src/async_delivery.rs`
- Modify: `crates/bot/src/cron.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Delete Stage 2 modules**

Delete:

```text
crates/bot/src/learning_episode.rs
crates/bot/src/learning_episode_tests.rs
crates/bot/src/learning_review.rs
crates/bot/src/learning_review_tests.rs
crates/bot/src/execution_events.rs
```

In `crates/bot/src/lib.rs`, remove:

```rust
// crates/bot/src/lib.rs
pub(crate) mod execution_events;
pub(crate) mod learning_episode;
pub(crate) mod learning_review;
```

- [ ] **Step 2: Remove startup drain scheduler**

In `crates/bot/src/lib.rs`, delete the block that creates:

```rust
// crates/bot/src/lib.rs
let learning_drain_scheduler = Arc::new(crate::learning_episode::DrainScheduler::noop());
let learning_drain_handle: Option<tokio::task::JoinHandle<()>> = None;
```

Remove `learning_drain_scheduler` and `learning_drain_handle` from downstream constructor calls. The final code should not mention `DrainScheduler`.

- [ ] **Step 3: Remove drain scheduler fields**

In `crates/bot/src/telegram/handler.rs`, remove this field from the context struct:

```rust
// crates/bot/src/telegram/handler.rs
pub(crate) learning_drain_scheduler: Arc<crate::learning_episode::DrainScheduler>,
```

In `crates/bot/src/telegram/worker.rs`, remove this field from `WorkerContext`:

```rust
// crates/bot/src/telegram/worker.rs
pub(crate) learning_drain_scheduler: Arc<crate::learning_episode::DrainScheduler>,
```

Remove corresponding constructor arguments in `dispatch.rs`, `background.rs`, `async_delivery.rs`, and `cron.rs`.

- [ ] **Step 4: Remove seed-capture callsites**

Remove functions and calls that only create legacy `learning_episodes` rows:

```rust
// crates/bot/src/background.rs
capture_background_learning_episode_seed(...)

// crates/bot/src/async_delivery.rs
capture_async_learning_episode_seed(...)

// crates/bot/src/cron.rs
capture_cron_learning_episode_seed(...)

// crates/bot/src/telegram/worker.rs
foreground_episode_seed_trigger_kind(...)
capture_foreground_learning_episode_seed(...)
```

After this step, the command below must return no hits:

```bash
devenv shell -- rg -n "capture_.*learning_episode|LearningEpisodeRuntime|DrainScheduler|learning_drain_scheduler|learning_episodes" crates/bot/src
```

- [ ] **Step 5: Remove legacy reply-signal and review-spawn plumbing**

In `crates/bot/src/telegram/worker.rs`, remove:

```rust
// crates/bot/src/telegram/worker.rs
reply_has_accepted_signal
maybe_spawn_learned_skill_review
clear_background_review_gate_on_shutdown
build_background_review_claude_command
run_background_learned_skill_review
record_successful_background_review
record_failed_background_review
```

`CcReply` should no longer contain `reply_has_accepted_signal`:

```rust
// crates/bot/src/telegram/worker.rs
pub(crate) struct CcReply {
    pub(crate) output: Option<ReplyOutput>,
    pub(crate) session_uuid: String,
    pub(crate) turn_id: u64,
    pub(crate) is_first_call: bool,
    pub(crate) prompt_mode: crate::cc::prompt::PromptMode,
    pub(crate) usage: crate::cc::stream::StreamUsage,
    pub(crate) wall_elapsed_ms: u64,
}
```

- [ ] **Step 6: Compile the bot and fix remaining references**

Run:

```bash
devenv shell -- cargo check -p right-bot
```

Expected: PASS. Any remaining errors should be direct references to deleted Stage 2 symbols; remove those references rather than reintroducing compatibility shims.

- [ ] **Step 7: Run targeted bot tests**

Run:

```bash
devenv shell -- cargo test -p right-bot worker_reply used_skill_receipts
devenv shell -- cargo test -p right-bot learning_prefilter learning_probe_writer learning_curator
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src
devenv shell -- git commit -m "refactor(bot): remove legacy stage two learning runtime"
```

## Task 4: Remove Legacy Dashboard Backend APIs

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/right-dashboard/src/read_model.rs`
- Delete: `crates/right-dashboard/src/read_model/learning_episodes.rs`
- Modify: `crates/right-dashboard/src/read_model/learning.rs`
- Modify: `crates/right-dashboard/src/api_types.rs`

- [ ] **Step 1: Add route-removal regression tests**

In `crates/bot/src/telegram/dashboard.rs`, replace the existing authorized episode tests with:

```rust
// crates/bot/src/telegram/dashboard.rs
#[tokio::test]
async fn legacy_learning_episode_routes_are_not_mounted() {
    let state = dashboard_state_for_test("alpha").await;
    let app = build_dashboard_router(state);

    for path in [
        "/dashboard/alpha/api/v1/knowledge/learning/episodes",
        "/dashboard/alpha/api/v1/knowledge/learning/episodes/1",
        "/dashboard/alpha/api/v1/learning/reports/1",
        "/dashboard/alpha/api/v1/knowledge/learning/reports/1",
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND, "{path}");
    }
}
```

Use the existing `test_state(agent_dir)` helper in the same test module and follow the nearby `oneshot` request pattern.

- [ ] **Step 2: Run the failing route test**

Run:

```bash
devenv shell -- cargo test -p right-bot legacy_learning_episode_routes_are_not_mounted -- --exact
```

Expected: FAIL because the routes are still mounted.

- [ ] **Step 3: Remove episode/report routes and handlers**

In `crates/bot/src/telegram/dashboard.rs`, remove route entries for:

```rust
// crates/bot/src/telegram/dashboard.rs
"/dashboard/{agent}/api/v1/knowledge/learning/episodes"
"/dashboard/{agent}/api/v1/knowledge/learning/episodes/{episode_id}"
"/dashboard/{agent}/api/v1/learning/reports/{report_id}"
"/dashboard/{agent}/api/v1/knowledge/learning/reports/{report_id}"
```

Remove handlers:

```rust
// crates/bot/src/telegram/dashboard.rs
handle_learning_episodes
handle_learning_episode_detail
handle_learning_report_detail
```

Remove imports for:

```rust
// crates/bot/src/telegram/dashboard.rs
LearningEpisodesInput
learning_episodes
learning_episode_detail
learning_report_detail
```

- [ ] **Step 4: Remove the learning episode read model**

Delete `crates/right-dashboard/src/read_model/learning_episodes.rs`.

In `crates/right-dashboard/src/read_model.rs`, remove:

```rust
// crates/right-dashboard/src/read_model.rs
#[path = "read_model/learning_episodes.rs"]
pub mod learning_episodes;
```

- [ ] **Step 5: Remove deprecated DTOs**

In `crates/right-dashboard/src/api_types.rs`, remove:

```rust
// crates/right-dashboard/src/api_types.rs
LearningReportSummary
LearningEpisodesResponse
LearningEpisodeSummary
LearningEpisodeDetailResponse
LearningReportDetailResponse
LearningEpisodeDetail
LearningSelectorDetail
LearningEvidenceSnippet
LearningReviewerDetail
```

Replace `LearningOverviewResponse` with a current-only shape:

```rust
// crates/right-dashboard/src/api_types.rs
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub capabilities: LearningCapabilities,
    pub lifecycle: LearningLifecycle,
    pub flow_nodes: Vec<LearningFlowNode>,
    pub flow_edges: Vec<LearningFlowEdge>,
    pub recent_learning_signals: Vec<LearningSignalPoint>,
    pub warnings: Vec<DashboardDataWarning>,
}
```

- [ ] **Step 6: Rebuild `learning_overview` around current tables**

In `crates/right-dashboard/src/read_model/learning.rs`, remove functions that query:

```rust
// crates/right-dashboard/src/read_model/learning.rs
skill_nudge_signals
learning_episodes
skill_review_reports
execution_events
```

Keep current-source helpers for:

```rust
// crates/right-dashboard/src/read_model/learning.rs
skill_learning_events
skill_lifecycle
curator_state
usage_events
conversation_messages
```

The top-level `learning_overview` return should build the current-only DTO:

```rust
// crates/right-dashboard/src/read_model/learning.rs
Ok(LearningOverviewResponse {
    agent: agent_name,
    generated_at,
    refresh_interval_secs: input.refresh_interval_secs,
    capabilities: learning_capabilities(),
    lifecycle,
    flow_nodes,
    flow_edges,
    recent_learning_signals,
    warnings,
})
```

- [ ] **Step 7: Add overview regression test without legacy tables**

In `crates/right-dashboard/src/read_model/learning.rs`, add:

```rust
// crates/right-dashboard/src/read_model/learning.rs
#[tokio::test]
async fn learning_overview_reads_current_sources_without_legacy_tables() {
    let (_dir, conn) = right_db::test_support::migrated_connection().await;
    for table in [
        "learning_episodes",
        "skill_nudge_signals",
        "skill_nudge_state",
        "skill_review_reports",
        "execution_events",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .await
            .unwrap();
        assert_eq!(exists, 0, "{table} must not exist after cleanup migration");
    }

    conn.execute(
        "INSERT INTO skill_learning_events (
            invocation_id, agent_name, action, skill_name, phase, status,
            hint_outcome, reason, message, summary, event_refs_json, created_at
         ) VALUES (
            'inv-1', 'right', 'create', 'rightx-current', 'finish', 'created',
            'applied_as_hinted', 'captured workflow', 'Created skill',
            'Reusable workflow', '[\"msg:1\"]', '2026-05-26T10:00:00Z'
         )",
        [],
    )
    .await
    .unwrap();

    let response = learning_overview(
        &conn,
        LearningOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-05-26T10:05:00Z".to_owned(),
            refresh_interval_secs: 30,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.agent, "right");
    assert!(
        response
            .recent_learning_signals
            .iter()
            .any(|point| point.skill_name.as_deref() == Some("rightx-current"))
    );
}
```

- [ ] **Step 8: Run dashboard backend tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_overview_reads_current_sources_without_legacy_tables -- --exact
devenv shell -- cargo test -p right-bot legacy_learning_episode_routes_are_not_mounted -- --exact
```

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs crates/right-dashboard/src
devenv shell -- git commit -m "refactor(dashboard): remove legacy learning history APIs"
```

## Task 5: Remove Legacy Dashboard Frontend Surfaces

**Files:**
- Modify: `crates/right-dashboard/frontend/src/api.ts`
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/App.vue`
- Modify: `crates/right-dashboard/frontend/src/views/KnowledgeView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`
- Delete: `crates/right-dashboard/frontend/src/views/learning/EpisodesView.vue`
- Modify: `crates/right-dashboard/static/dashboard/**`

- [ ] **Step 1: Remove legacy API client calls**

In `crates/right-dashboard/frontend/src/api.ts`, remove imports and functions for:

```ts
// crates/right-dashboard/frontend/src/api.ts
LearningEpisodesResponse
LearningEpisodeDetailResponse
LearningReportDetailResponse
learningEpisodes
learningEpisodeDetail
learningReportDetail
```

Keep:

```ts
// crates/right-dashboard/frontend/src/api.ts
export function learningOverview(): Promise<LearningOverviewResponse> {
  return requestJson<LearningOverviewResponse>('api/v1/knowledge/learning/overview')
}
```

- [ ] **Step 2: Remove legacy frontend types**

In `crates/right-dashboard/frontend/src/types.ts`, remove interfaces for:

```ts
// crates/right-dashboard/frontend/src/types.ts
LearningReportSummary
LearningEpisodesResponse
LearningEpisodeSummary
LearningEpisodeDetailResponse
LearningReportDetailResponse
LearningEpisodeDetail
LearningSelectorDetail
LearningEvidenceSnippet
LearningReviewerDetail
```

Update `LearningOverviewResponse` to match Rust:

```ts
// crates/right-dashboard/frontend/src/types.ts
export interface LearningOverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  capabilities: LearningCapabilities
  lifecycle: LearningLifecycle
  flow_nodes: LearningFlowNode[]
  flow_edges: LearningFlowEdge[]
  recent_learning_signals: LearningSignalPoint[]
  warnings: DashboardDataWarning[]
}
```

- [ ] **Step 3: Remove episode/report state from the app**

In `crates/right-dashboard/frontend/src/App.vue`, remove state, imports, and handlers for:

```ts
// crates/right-dashboard/frontend/src/App.vue
LearningEpisodeDetailResponse
LearningEpisodeSummary
LearningEpisodesResponse
LearningReportDetailResponse
LearningReportSummary
learningEpisodes
learningEpisodeDetail
learningReportDetail
learningEpisodesData
selectedEpisode
selectedEpisodeId
selectedLearningReport
selectedLearningReportId
loadingEpisode
loadingReport
episodeError
reportError
selectEpisode
selectLearningReport
```

`refreshKnowledge` should become:

```ts
// crates/right-dashboard/frontend/src/App.vue
async function refreshKnowledge(): Promise<void> {
  if (activeKnowledgeTab.value === 'skills') {
    await refreshSkills()
    return
  }

  await guarded(async () => {
    learningData.value = await learningOverview()
  })
}
```

- [ ] **Step 4: Replace Knowledge subtabs**

In `crates/right-dashboard/frontend/src/App.vue`, set:

```ts
// crates/right-dashboard/frontend/src/App.vue
type KnowledgeTab = 'learning' | 'skills'
const activeKnowledgeTab = ref<KnowledgeTab>('learning')
```

In `crates/right-dashboard/frontend/src/views/KnowledgeView.vue`, remove `EpisodesView` and rename the reports tab to current learning:

```vue
<!-- crates/right-dashboard/frontend/src/views/KnowledgeView.vue -->
<nav class="subtabs" aria-label="Knowledge views">
  <button type="button" class="tab-button" :class="{ active: activeSubtab === 'learning' }" @click="emit('setSubtab', 'learning')">Learning</button>
  <button type="button" class="tab-button" :class="{ active: activeSubtab === 'skills' }" @click="emit('setSubtab', 'skills')">Skills</button>
</nav>

<ReportsView
  v-if="activeSubtab === 'learning'"
  :learning="learning"
/>
<SkillsView
  v-else
  :skills="skills"
  :selected-skill="selectedSkill"
  :selected-skill-name="selectedSkillName"
  :loading="loadingSkill"
  :error="skillError"
  @select-skill="emit('selectSkill', $event)"
  @skill-pinned="emit('skillPinned', $event)"
/>
```

- [ ] **Step 5: Make `ReportsView.vue` current-data only**

In `crates/right-dashboard/frontend/src/views/learning/ReportsView.vue`, remove report list/detail selection. Keep current learning signals and lifecycle metrics. The template should not reference `recent_reports`, `funnel.reports_total_24h`, or `quality.candidate_rate`.

Use this current-data card set:

```vue
<!-- crates/right-dashboard/frontend/src/views/learning/ReportsView.vue -->
<MetricCard label="Created 7d" :value="learning?.lifecycle.created_7d ?? 0" tone="success" />
<MetricCard label="Updated 7d" :value="learning?.lifecycle.updated_7d ?? 0" tone="active" />
<MetricCard label="Failed 7d" :value="learning?.lifecycle.failed_or_aborted_7d ?? 0" tone="danger" />
```

- [ ] **Step 6: Typecheck frontend**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

Expected: PASS.

- [ ] **Step 7: Build regenerated static dashboard assets**

Run:

```bash
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: PASS and changes under `crates/right-dashboard/static/dashboard/**`.

- [ ] **Step 8: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "refactor(dashboard): remove legacy learning frontend"
```

## Task 6: Keep Legacy Config Compatibility

**Files:**
- Modify: `crates/right-agent-config/src/lib.rs`
- Inspect: `crates/right/src/wizard.rs` — confirm deprecated fields still initialize to `None`.
- Inspect: `crates/right-agent/src/agent/types.rs` — confirm deprecated-field compatibility assertions still match warning-only behavior.

- [ ] **Step 1: Confirm deprecated keys still deserialize**

Keep this test in `crates/right-agent-config/src/lib.rs`:

```rust
// crates/right-agent-config/src/lib.rs
#[test]
fn learning_config_deprecated_fields_are_ignored() {
    let yaml = r#"
fork_probe_enabled: true
fork_probe_model: claude-opus-4-7
background_review_enabled: true
episode_settle_seconds: 60
circuit_failure_threshold: 5
circuit_cooldown_minutes: 60
episode_selector_max_budget_usd: 0.10
episode_selector_model: claude-haiku-4-5
max_daily_budget_usd: 2.50
prefilter_enabled: false
"#;
    let cfg: LearningConfig = serde_saphyr::from_str(yaml).unwrap();
    assert_eq!(cfg.max_daily_budget_usd, 2.50);
    assert!(!cfg.prefilter_enabled);
    assert!(
        cfg.probe_writer_enabled,
        "probe_writer_enabled defaults to true"
    );
}
```

- [ ] **Step 2: Keep deprecated fields in `LearningConfig`**

Do not remove these fields from `crates/right-agent-config/src/lib.rs`:

```rust
// crates/right-agent-config/src/lib.rs
pub fork_probe_enabled: Option<bool>,
pub fork_probe_model: Option<String>,
#[serde(default, rename = "probe_model")]
pub legacy_probe_model: Option<String>,
pub background_review_enabled: Option<bool>,
pub episode_selector_model: Option<String>,
pub episode_selector_max_budget_usd: Option<f64>,
pub episode_settle_seconds: Option<u64>,
pub circuit_failure_threshold: Option<u32>,
pub circuit_cooldown_minutes: Option<u32>,
```

Keep `warn_on_deprecated` so old files warn but do not fail.

- [ ] **Step 3: Verify config tests**

Run:

```bash
devenv shell -- cargo test -p right-agent-config learning_config_deprecated_fields_are_ignored -- --exact
devenv shell -- cargo test -p right-agent agent_config_accepts_deprecated_learning_fields
```

Expected: PASS.

- [ ] **Step 4: Commit config compatibility edits**

When this task changes files, run:

```bash
devenv shell -- git add crates/right-agent-config/src/lib.rs crates/right/src/wizard.rs crates/right-agent/src/agent/types.rs
devenv shell -- git commit -m "test(config): preserve legacy learning key compatibility"
```

## Task 7: Update Architecture And Prompt Docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/mcp.md`

- [ ] **Step 1: Remove Stage 2 architecture references**

In `ARCHITECTURE.md`, remove paragraphs that say Stage 2 selector/reviewer is deprecated but retained. Replace them with:

```markdown
<!-- ARCHITECTURE.md -->
The only learning runtime is the prefilter/probe-writer/curator pipeline. The
old Stage 2 selector/reviewer, learning episode tables, nudge-signal gate, and
review reports have been removed from runtime and schema. Deprecated
`agent.yaml` learning keys are accepted only for upgrade compatibility and
warn at load time.
```

- [ ] **Step 2: Remove historical report claims from prompt docs**

In `PROMPT_SYSTEM.md`, remove the paragraph that says historical Stage 2 reports remain dashboard data. Keep the text that current replies use `used_skill_receipts` and legacy reply fields are ignored.

- [ ] **Step 3: Update satellite docs**

In `docs/architecture/modules.md`, replace the dashboard learning module description with:

```markdown
<!-- docs/architecture/modules.md -->
- `read_model/learning.rs` — learned-skill overview projections over
  `skill_learning_events`, `skill_lifecycle`, `curator_state`, usage, and
  trusted conversation data. It must not query removed Stage 2 tables.
```

In `docs/architecture/sessions.md` and `docs/architecture/mcp.md`, remove report-only `BackgroundReview` exceptions if the code is gone. Current background learning kinds are only `ProbeWriter` and `Curator`.

- [ ] **Step 4: Verify docs no longer describe legacy dashboard data**

Run:

```bash
devenv shell -- rg -n "learning_episodes|skill_nudge_signals|skill_review_reports|Stage 2|selector/reviewer|BackgroundReview" ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture
```

Expected: only explicit statements that the removed Stage 2 system no longer exists, or no hits. There must be no statement that the dashboard reads historical episode/report data.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture/modules.md docs/architecture/sessions.md docs/architecture/mcp.md
devenv shell -- git commit -m "docs(learning): document current-only learning pipeline"
```

## Task 8: Whole-Repo Legacy Reference Sweep

**Files:**
- Modify only files with confirmed stale references from the searches below.

- [ ] **Step 1: Search removed table references**

Run:

```bash
devenv shell -- rg -n "learning_episodes|skill_nudge_signals|skill_nudge_state|skill_review_reports|execution_events" crates ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture
```

Expected: no runtime/dashboard references. Allowed hits are old immutable `docs/superpowers/specs/**` and `docs/superpowers/plans/**` history, not active architecture or code.

- [ ] **Step 2: Search removed Stage 2 symbols**

Run:

```bash
devenv shell -- rg -n "DrainScheduler|LearningEpisodeRuntime|learning_episode|learning_review|ReviewGate|ReviewStatus|SkillReviewReport|NudgeSignal|background_review_enabled" crates ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture
```

Expected: `background_review_enabled` may remain only in config compatibility code/tests/docs. Other Stage 2 symbols should be gone from active code.

- [ ] **Step 3: Fix stale references**

For each disallowed active-code hit, delete the reference rather than adding compatibility shims. Do not edit historical specs/plans unless they are imported by active docs.

- [ ] **Step 4: Commit stale-reference edits**

When this task changes files, run:

```bash
devenv shell -- git add crates ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture
devenv shell -- git commit -m "refactor(learning): remove stale legacy references"
```

## Task 9: Final Verification

**Files:**
- Read: full workspace

- [ ] **Step 1: Run targeted package checks**

Run:

```bash
devenv shell -- cargo test -p right-db legacy_learning_cleanup
devenv shell -- cargo test -p right-agent learned_skills
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard
devenv shell -- cargo test -p right-agent-config learning_config_deprecated_fields_are_ignored
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: PASS.

- [ ] **Step 2: Run full mandatory workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Check final diff**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git diff --stat HEAD
```

Expected: only intended cleanup changes remain after the last commit. Generated dashboard assets may be present if the frontend changed.

- [ ] **Step 4: Commit generated assets after final verification**

When `npm run build` changes static assets after the previous frontend commit, run:

```bash
devenv shell -- git add crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "build(dashboard): regenerate learning cleanup assets"
```
