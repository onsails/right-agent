# Dashboard Learned-Skill Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only Learning view to the Telegram Mini App dashboard that explains why `rightx-*` learned skills are or are not being created.

**Architecture:** Build a separate learning read-model slice in `right-dashboard`, expose authenticated bot-owned `/api/v1/learning/*` routes, and add a Vue Learning tab gated by backend capabilities. Keep cron overview unchanged, load evidence snippets only on report detail selection, and expose no mutating endpoints.

**Tech Stack:** Rust 2024, `right-dashboard`, `right-bot`, SQLite via `rusqlite`, Axum 0.8, Vue 3 + Vite + TypeScript, checked-in static dashboard assets.

---

## Important Base Branch

This feature depends on the Telegram Mini App dashboard that exists on `master`.
Do not implement it from a branch that lacks `crates/right-dashboard/`.

## Source Spec

Implement:

`docs/superpowers/specs/2026-05-20-dashboard-learned-skill-metrics-design.md`

## File Structure

- Modify `crates/right-dashboard/src/api_types.rs`
  - Extend `DashboardFeatures`.
  - Add learning overview/detail DTOs.
- Modify `crates/right-dashboard/src/read_model.rs`
  - Keep existing cron functions.
  - Add `#[path = "read_model/learning.rs"] pub mod learning;`.
- Create `crates/right-dashboard/src/read_model/learning.rs`
  - Own all learned-skill dashboard SQL and evidence snippet projection.
  - Keep helper tests local to this module.
- Modify `crates/bot/src/telegram/dashboard.rs`
  - Import learning read-model helpers.
  - Add authenticated learning routes.
  - Return learning capability flags from bootstrap.
- Modify `crates/right-dashboard/frontend/src/types.ts`
  - Mirror backend learning DTOs.
- Modify `crates/right-dashboard/frontend/src/api.ts`
  - Add `learningOverview()` and `learningReportDetail(reportId)`.
- Modify `crates/right-dashboard/frontend/src/App.vue`
  - Add Cron/Learning view switch.
  - Add Learning funnel, report list, quality/health panel, and report detail.
- Modify generated assets under `crates/right-dashboard/static/dashboard/`
  - Rebuild with Vite after frontend changes.
- Modify `docs/architecture/modules.md`
  - Document `read_model/learning.rs` and Learning tab responsibilities.
- Modify `docs/architecture/lifecycle.md`
  - Mention dashboard's authenticated learning read-only endpoints.

## Verification Cadence

Use targeted checks while implementing:

```bash
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Final verification after all tasks:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Task 1: Worktree And Baseline

**Files:**
- Read: `docs/superpowers/specs/2026-05-20-dashboard-learned-skill-metrics-design.md`
- Read: `ARCHITECTURE.md`
- Read: `docs/architecture/modules.md`
- Read: `docs/architecture/lifecycle.md`
- Read: `crates/right-dashboard/src/api_types.rs`
- Read: `crates/right-dashboard/src/read_model.rs`
- Read: `crates/bot/src/telegram/dashboard.rs`
- Read: `crates/right-dashboard/frontend/src/App.vue`

- [ ] **Step 1: Create an implementation worktree from `master`**

Run from the repo root:

```bash
devenv shell -- git fetch origin master
devenv shell -- git worktree add .worktrees/dashboard-learned-skill-metrics -b feat/dashboard-learned-skill-metrics origin/master
```

Expected: worktree exists at `.worktrees/dashboard-learned-skill-metrics` and contains `crates/right-dashboard/`.

- [ ] **Step 2: Enter the worktree and verify branch state**

Run:

```bash
cd .worktrees/dashboard-learned-skill-metrics
devenv shell -- git status --short --branch
```

Expected: clean branch `feat/dashboard-learned-skill-metrics`.

- [ ] **Step 3: Re-read required architecture docs**

Run:

```bash
devenv shell -- sed -n '1,260p' docs/superpowers/specs/2026-05-20-dashboard-learned-skill-metrics-design.md
devenv shell -- sed -n '1,220p' docs/architecture/modules.md
devenv shell -- sed -n '1,120p' docs/architecture/lifecycle.md
```

Expected: docs are readable. Note any drift before editing.

- [ ] **Step 4: Run narrow baseline checks**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

Expected: pass before edits. If any command fails, record the failing test or typecheck error in task notes before continuing.

## Task 2: Learning API DTOs

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`

- [ ] **Step 1: Add the failing DTO contract test**

Append this test module to `crates/right-dashboard/src/api_types.rs`:

```rust
#[cfg(test)]
mod learning_tests {
    use super::*;

    #[test]
    fn learning_overview_serializes_expected_shape() {
        let response = LearningOverviewResponse {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            refresh_interval_secs: 5,
            capabilities: LearningCapabilities {
                learning_metrics: true,
                learning_evidence_snippets: true,
                learning_commands: false,
            },
            funnel: LearningFunnel {
                signals_accepted_24h: 2,
                episodes_pending_24h: 1,
                episodes_selecting_24h: 0,
                episodes_selected_24h: 0,
                episodes_reviewing_24h: 0,
                episodes_reviewed_24h: 1,
                episodes_no_episode_24h: 0,
                episodes_failed_24h: 0,
                reports_total_24h: 1,
                create_candidates_24h: 1,
                update_candidates_24h: 0,
                nothing_to_learn_24h: 0,
                failed_reviews_24h: 0,
                foreground_created_or_updated_7d: 1,
            },
            quality: LearningQuality {
                candidate_rate: Some(1.0),
                nothing_to_learn_rate: Some(0.0),
                create_count_24h: 1,
                update_count_24h: 0,
                high_confidence_count_24h: 1,
                medium_confidence_count_24h: 0,
                low_confidence_count_24h: 0,
                failed_count_24h: 0,
            },
            health: LearningHealth {
                review_running: false,
                daily_review_count: 1,
                daily_limit: 12,
                creation_review_interval: 15,
                tool_iters_since_review: 3,
                turns_since_review: 1,
                skill_issue_hints_since_review: 0,
                last_review_status: Some("create_candidate".to_owned()),
                last_review_at: Some("2026-05-20T11:00:00Z".to_owned()),
                possibly_stuck: false,
            },
            lifecycle: LearningLifecycle {
                created_7d: 1,
                updated_7d: 0,
                failed_or_aborted_7d: 0,
                recent_successful_events: vec![LearningEventSummary {
                    skill_name: "rightx-oauth-debugging".to_owned(),
                    action: "create".to_owned(),
                    status: "created".to_owned(),
                    message: Some("Learned OAuth callback verification.".to_owned()),
                    summary: Some("Reusable OAuth setup workflow.".to_owned()),
                    created_at: "2026-05-20T10:00:00Z".to_owned(),
                }],
                candidate_skill_names_7d: vec!["rightx-oauth-debugging".to_owned()],
            },
            recent_reports: vec![LearningReportSummary {
                id: 7,
                status: "create_candidate".to_owned(),
                confidence: "high".to_owned(),
                trigger_kind: "learning_signal".to_owned(),
                candidate_skill_name: Some("rightx-oauth-debugging".to_owned()),
                candidate_summary: Some("Verify OAuth callback setup.".to_owned()),
                telegram_notified: true,
                created_at: "2026-05-20T11:00:00Z".to_owned(),
            }],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["capabilities"]["learning_metrics"], true);
        assert_eq!(value["capabilities"]["learning_commands"], false);
        assert_eq!(value["funnel"]["create_candidates_24h"], 1);
        assert_eq!(value["quality"]["candidate_rate"], 1.0);
        assert_eq!(value["recent_reports"][0]["candidate_skill_name"], "rightx-oauth-debugging");
    }

    #[test]
    fn learning_report_detail_serializes_missing_snippet() {
        let response = LearningReportDetailResponse {
            report: LearningReportSummary {
                id: 9,
                status: "nothing_to_learn".to_owned(),
                confidence: "medium".to_owned(),
                trigger_kind: "effort_threshold".to_owned(),
                candidate_skill_name: None,
                candidate_summary: None,
                telegram_notified: false,
                created_at: "2026-05-20T11:00:00Z".to_owned(),
            },
            episode: Some(LearningEpisodeDetail {
                id: 4,
                kind: "foreground_thread".to_owned(),
                seed_trigger_kind: "effort_threshold".to_owned(),
                status: "reviewed".to_owned(),
                start_ref: Some("msg:1".to_owned()),
                end_ref: Some("exec:2".to_owned()),
                boundary_rationale: Some("Selected compact setup workflow.".to_owned()),
                confidence: Some("medium".to_owned()),
                context_incomplete: false,
            }),
            selector: Some(LearningSelectorDetail {
                model: Some("claude-sonnet-4-6".to_owned()),
                boundary_rationale: Some("Selected compact setup workflow.".to_owned()),
                selected_message_refs: vec!["msg:1".to_owned()],
                selected_execution_event_refs: vec!["exec:2".to_owned()],
            }),
            evidence: vec![LearningEvidenceSnippet {
                ref_id: "msg:404".to_owned(),
                source: "message".to_owned(),
                available: false,
                trust_label: None,
                role: None,
                event_kind: None,
                tool_name: None,
                created_at: None,
                text: None,
            }],
            reviewer: LearningReviewerDetail {
                status: "nothing_to_learn".to_owned(),
                confidence: "medium".to_owned(),
                candidate_skill_name: None,
                candidate_summary: None,
                evidence_refs: vec!["msg:404".to_owned()],
                user_notice_present: false,
            },
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["evidence"][0]["available"], false);
        assert!(value["evidence"][0]["text"].is_null());
        assert_eq!(value["reviewer"]["user_notice_present"], false);
    }
}
```

- [ ] **Step 2: Run the failing DTO test**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_tests
```

Expected: fail because the learning DTO types and fields do not exist.

- [ ] **Step 3: Add learning DTOs**

In `crates/right-dashboard/src/api_types.rs`, extend `DashboardFeatures`:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardFeatures {
    pub readonly: bool,
    pub commands_enabled: bool,
    pub learning_metrics: bool,
    pub learning_evidence_snippets: bool,
    pub learning_commands: bool,
}
```

Add these DTOs below `LogExcerpt`:

```rust
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningCapabilities {
    pub learning_metrics: bool,
    pub learning_evidence_snippets: bool,
    pub learning_commands: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub capabilities: LearningCapabilities,
    pub funnel: LearningFunnel,
    pub quality: LearningQuality,
    pub health: LearningHealth,
    pub lifecycle: LearningLifecycle,
    pub recent_reports: Vec<LearningReportSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFunnel {
    pub signals_accepted_24h: i64,
    pub episodes_pending_24h: i64,
    pub episodes_selecting_24h: i64,
    pub episodes_selected_24h: i64,
    pub episodes_reviewing_24h: i64,
    pub episodes_reviewed_24h: i64,
    pub episodes_no_episode_24h: i64,
    pub episodes_failed_24h: i64,
    pub reports_total_24h: i64,
    pub create_candidates_24h: i64,
    pub update_candidates_24h: i64,
    pub nothing_to_learn_24h: i64,
    pub failed_reviews_24h: i64,
    pub foreground_created_or_updated_7d: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningQuality {
    pub candidate_rate: Option<f64>,
    pub nothing_to_learn_rate: Option<f64>,
    pub create_count_24h: i64,
    pub update_count_24h: i64,
    pub high_confidence_count_24h: i64,
    pub medium_confidence_count_24h: i64,
    pub low_confidence_count_24h: i64,
    pub failed_count_24h: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningHealth {
    pub review_running: bool,
    pub daily_review_count: i64,
    pub daily_limit: i64,
    pub creation_review_interval: i64,
    pub tool_iters_since_review: i64,
    pub turns_since_review: i64,
    pub skill_issue_hints_since_review: i64,
    pub last_review_status: Option<String>,
    pub last_review_at: Option<String>,
    pub possibly_stuck: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningLifecycle {
    pub created_7d: i64,
    pub updated_7d: i64,
    pub failed_or_aborted_7d: i64,
    pub recent_successful_events: Vec<LearningEventSummary>,
    pub candidate_skill_names_7d: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEventSummary {
    pub skill_name: String,
    pub action: String,
    pub status: String,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningReportSummary {
    pub id: i64,
    pub status: String,
    pub confidence: String,
    pub trigger_kind: String,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub telegram_notified: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningReportDetailResponse {
    pub report: LearningReportSummary,
    pub episode: Option<LearningEpisodeDetail>,
    pub selector: Option<LearningSelectorDetail>,
    pub evidence: Vec<LearningEvidenceSnippet>,
    pub reviewer: LearningReviewerDetail,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEpisodeDetail {
    pub id: i64,
    pub kind: String,
    pub seed_trigger_kind: String,
    pub status: String,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub boundary_rationale: Option<String>,
    pub confidence: Option<String>,
    pub context_incomplete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningSelectorDetail {
    pub model: Option<String>,
    pub boundary_rationale: Option<String>,
    pub selected_message_refs: Vec<String>,
    pub selected_execution_event_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEvidenceSnippet {
    pub ref_id: String,
    pub source: String,
    pub available: bool,
    pub trust_label: Option<String>,
    pub role: Option<String>,
    pub event_kind: Option<String>,
    pub tool_name: Option<String>,
    pub created_at: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningReviewerDetail {
    pub status: String,
    pub confidence: String,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub evidence_refs: Vec<String>,
    pub user_notice_present: bool,
}
```

- [ ] **Step 4: Fix bootstrap feature construction compile errors**

In `crates/bot/src/telegram/dashboard.rs`, update the existing `DashboardFeatures` construction in `handle_bootstrap`:

```rust
features: DashboardFeatures {
    readonly: true,
    commands_enabled: false,
    learning_metrics: true,
    learning_evidence_snippets: true,
    learning_commands: false,
},
```

- [ ] **Step 5: Verify DTO tests pass**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_tests
```

Expected: pass.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(dashboard): add learning metrics DTOs"
```

## Task 3: Learning Overview Read Model

**Files:**
- Modify: `crates/right-dashboard/src/read_model.rs`
- Create: `crates/right-dashboard/src/read_model/learning.rs`

- [ ] **Step 1: Expose the learning read-model module**

In `crates/right-dashboard/src/read_model.rs`, add this near the top after imports:

```rust
#[path = "read_model/learning.rs"]
pub mod learning;
```

- [ ] **Step 2: Create the failing overview tests**

Create `crates/right-dashboard/src/read_model/learning.rs` with this initial content:

```rust
use crate::api_types::{
    LearningCapabilities, LearningEventSummary, LearningFunnel, LearningHealth, LearningLifecycle,
    LearningOverviewResponse, LearningQuality, LearningReportSummary,
};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::ReadModelError;

pub const LEARNING_REVIEW_DAILY_LIMIT: i64 = 12;
const RECENT_REPORT_LIMIT: i64 = 20;
const RECENT_EVENT_LIMIT: i64 = 10;
const CANDIDATE_NAME_LIMIT: i64 = 20;

pub struct LearningOverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
}

pub fn learning_overview(
    _conn: &Connection,
    _input: LearningOverviewInput,
) -> Result<LearningOverviewResponse, ReadModelError> {
    panic!("red test: learning overview read model is not wired")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(dir.path(), true).expect("open db");
        (dir, conn)
    }

    fn input() -> LearningOverviewInput {
        LearningOverviewInput {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            refresh_interval_secs: 5,
        }
    }

    #[test]
    fn learning_overview_builds_funnel_quality_health_and_lifecycle() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, tool_iters_since_review, turns_since_review,
                skill_issue_hints_since_review, last_review_at, review_running,
                creation_review_interval, daily_review_count, daily_review_date,
                last_review_status
             ) VALUES (
                'right', 6, 2, 1, '2026-05-20T11:00:00Z', 0,
                15, 4, '2026-05-20', 'nothing_to_learn'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_nudge_signals (
                invocation_id, agent_name, root_session_id, chat_id, thread_id,
                signal_kind, payload_json, accepted_at
             ) VALUES (
                'inv-1', 'right', 'session-1', 10, 20,
                'learning', '{}', '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, message_refs_json,
                execution_event_refs_json, selector_output_json, ready_after,
                created_at, updated_at
             ) VALUES (
                1, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, '[]', '[]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                root_session_id, chat_id, thread_id, trigger_kind, status,
                confidence, candidate_skill_name, candidate_summary,
                evidence_refs_json, review_output_json, telegram_notified,
                created_at
             ) VALUES (
                7, 'right', 'inv-1', 1, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:1\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"evidence_refs\":[\"msg:1\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_learning_events (
                invocation_id, agent_name, action, skill_name, phase, status,
                message, summary, event_refs_json, created_at
             ) VALUES (
                'inv-2', 'right', 'create', 'rightx-oauth-debugging', 'finish',
                'created', 'Learned OAuth callback verification.',
                'Reusable OAuth setup workflow.', '[]',
                '2026-05-20T11:10:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.funnel.signals_accepted_24h, 1);
        assert_eq!(response.funnel.episodes_reviewed_24h, 1);
        assert_eq!(response.funnel.create_candidates_24h, 1);
        assert_eq!(response.funnel.foreground_created_or_updated_7d, 1);
        assert_eq!(response.quality.candidate_rate, Some(1.0));
        assert_eq!(response.quality.nothing_to_learn_rate, Some(0.0));
        assert!(!response.health.review_running);
        assert_eq!(response.health.daily_review_count, 4);
        assert_eq!(response.lifecycle.created_7d, 1);
        assert_eq!(response.lifecycle.candidate_skill_names_7d, vec!["rightx-oauth-debugging"]);
        assert_eq!(response.recent_reports[0].id, 7);
    }

    #[test]
    fn learning_overview_rates_are_null_without_non_failed_reports() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                'right', 'inv-1', 'effort_threshold', 'failed',
                'low', '[]', '{}', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert_eq!(response.quality.candidate_rate, None);
        assert_eq!(response.quality.nothing_to_learn_rate, None);
        assert_eq!(response.quality.failed_count_24h, 1);
    }

    #[test]
    fn learning_overview_detects_old_reviewing_episode_as_possibly_stuck() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_nudge_state (
                agent_name, review_running, creation_review_interval,
                daily_review_count
             ) VALUES ('right', 1, 15, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, ready_after,
                created_at, updated_at
             ) VALUES (
                'right', 'foreground_thread', 'learning_signal', 'inv:stuck',
                'reviewing', '[]', '[]', '2026-05-20T09:00:00Z',
                '2026-05-20T09:00:00Z', '2026-05-20T09:05:00Z'
             )",
            [],
        )
        .unwrap();

        let response = learning_overview(&conn, input()).unwrap();

        assert!(response.health.review_running);
        assert!(response.health.possibly_stuck);
    }
}
```

- [ ] **Step 3: Run the failing overview tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_overview
```

Expected: fail because `learning_overview()` still panics with the red-test stub.

- [ ] **Step 4: Add helper functions**

In `crates/right-dashboard/src/read_model/learning.rs`, add these helpers above `learning_overview` before replacing the red-test stub:

```rust
fn parse_generated_at(value: &str) -> Result<DateTime<Utc>, ReadModelError> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn window_start(generated_at: &str, duration: Duration) -> Result<String, ReadModelError> {
    Ok((parse_generated_at(generated_at)? - duration).to_rfc3339())
}

fn count_since(
    conn: &Connection,
    sql: &str,
    agent: &str,
    since: &str,
) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(sql, params![agent, since], |row| row.get(0))?)
}

fn rate(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator == 0 {
        None
    } else {
        Some(numerator as f64 / denominator as f64)
    }
}

fn learning_capabilities() -> LearningCapabilities {
    LearningCapabilities {
        learning_metrics: true,
        learning_evidence_snippets: true,
        learning_commands: false,
    }
}
```

- [ ] **Step 5: Implement `learning_overview`**

Replace `learning_overview()` with:

```rust
pub fn learning_overview(
    conn: &Connection,
    input: LearningOverviewInput,
) -> Result<LearningOverviewResponse, ReadModelError> {
    let since_24h = window_start(&input.generated_at, Duration::hours(24))?;
    let since_7d = window_start(&input.generated_at, Duration::days(7))?;
    let agent = input.agent.as_str();

    let signals_accepted_24h = count_since(
        conn,
        "SELECT COUNT(*) FROM skill_nudge_signals WHERE agent_name=?1 AND accepted_at >= ?2",
        agent,
        &since_24h,
    )?;

    let episode_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM learning_episodes
             WHERE agent_name=?1 AND status=?2 AND created_at >= ?3",
            params![agent, status, since_24h],
            |row| row.get(0),
        )?)
    };

    let report_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM skill_review_reports
             WHERE agent_name=?1 AND status=?2 AND created_at >= ?3",
            params![agent, status, since_24h],
            |row| row.get(0),
        )?)
    };

    let reports_total_24h = count_since(
        conn,
        "SELECT COUNT(*) FROM skill_review_reports WHERE agent_name=?1 AND created_at >= ?2",
        agent,
        &since_24h,
    )?;
    let create_candidates_24h = report_count("create_candidate")?;
    let update_candidates_24h = report_count("update_candidate")?;
    let nothing_to_learn_24h = report_count("nothing_to_learn")?;
    let failed_reviews_24h = report_count("failed")?;
    let non_failed_reports = create_candidates_24h + update_candidates_24h + nothing_to_learn_24h;
    let foreground_created_or_updated_7d = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2",
        params![agent, since_7d],
        |row| row.get(0),
    )?;

    let quality = LearningQuality {
        candidate_rate: rate(create_candidates_24h + update_candidates_24h, non_failed_reports),
        nothing_to_learn_rate: rate(nothing_to_learn_24h, non_failed_reports),
        create_count_24h: create_candidates_24h,
        update_count_24h: update_candidates_24h,
        high_confidence_count_24h: confidence_count(conn, agent, &since_24h, "high")?,
        medium_confidence_count_24h: confidence_count(conn, agent, &since_24h, "medium")?,
        low_confidence_count_24h: confidence_count(conn, agent, &since_24h, "low")?,
        failed_count_24h: failed_reviews_24h,
    };

    Ok(LearningOverviewResponse {
        agent: input.agent,
        generated_at: input.generated_at.clone(),
        refresh_interval_secs: input.refresh_interval_secs,
        capabilities: learning_capabilities(),
        funnel: LearningFunnel {
            signals_accepted_24h,
            episodes_pending_24h: episode_count("pending")?,
            episodes_selecting_24h: episode_count("selecting")?,
            episodes_selected_24h: episode_count("selected")?,
            episodes_reviewing_24h: episode_count("reviewing")?,
            episodes_reviewed_24h: episode_count("reviewed")?,
            episodes_no_episode_24h: episode_count("no_episode")?,
            episodes_failed_24h: episode_count("failed")?,
            reports_total_24h,
            create_candidates_24h,
            update_candidates_24h,
            nothing_to_learn_24h,
            failed_reviews_24h,
            foreground_created_or_updated_7d,
        },
        quality,
        health: learning_health(conn, agent, &input.generated_at)?,
        lifecycle: learning_lifecycle(conn, agent, &since_7d)?,
        recent_reports: recent_reports(conn, agent)?,
    })
}

fn confidence_count(
    conn: &Connection,
    agent: &str,
    since: &str,
    confidence: &str,
) -> Result<i64, ReadModelError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM skill_review_reports
         WHERE agent_name=?1 AND confidence=?2 AND created_at >= ?3",
        params![agent, confidence, since],
        |row| row.get(0),
    )?)
}

fn learning_health(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<LearningHealth, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT review_running, daily_review_count, creation_review_interval,
                    tool_iters_since_review, turns_since_review,
                    skill_issue_hints_since_review, last_review_status, last_review_at
             FROM skill_nudge_state WHERE agent_name=?1",
            params![agent],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let (
        review_running,
        daily_review_count,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
    ) = row.unwrap_or((0, 0, 15, 0, 0, 0, None, None));

    Ok(LearningHealth {
        review_running: review_running != 0,
        daily_review_count,
        daily_limit: LEARNING_REVIEW_DAILY_LIMIT,
        creation_review_interval,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        last_review_status,
        last_review_at,
        possibly_stuck: possibly_stuck(conn, agent, generated_at)?,
    })
}

fn possibly_stuck(
    conn: &Connection,
    agent: &str,
    generated_at: &str,
) -> Result<bool, ReadModelError> {
    let cutoff = (parse_generated_at(generated_at)? - Duration::minutes(10)).to_rfc3339();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM learning_episodes
         WHERE agent_name=?1 AND status='reviewing' AND updated_at < ?2",
        params![agent, cutoff],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn learning_lifecycle(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<LearningLifecycle, ReadModelError> {
    let status_count = |status: &str| -> Result<i64, ReadModelError> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM skill_learning_events
             WHERE agent_name=?1 AND phase='finish' AND status=?2 AND created_at >= ?3",
            params![agent, status, since_7d],
            |row| row.get(0),
        )?)
    };
    let failed_or_aborted_7d = conn.query_row(
        "SELECT COUNT(*) FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('failed','aborted')
           AND created_at >= ?2",
        params![agent, since_7d],
        |row| row.get(0),
    )?;

    Ok(LearningLifecycle {
        created_7d: status_count("created")?,
        updated_7d: status_count("updated")?,
        failed_or_aborted_7d,
        recent_successful_events: recent_successful_events(conn, agent, since_7d)?,
        candidate_skill_names_7d: candidate_skill_names(conn, agent, since_7d)?,
    })
}

fn recent_successful_events(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<Vec<LearningEventSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT skill_name, action, status, message, summary, created_at
         FROM skill_learning_events
         WHERE agent_name=?1
           AND phase='finish'
           AND status IN ('created','updated')
           AND created_at >= ?2
         ORDER BY created_at DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, since_7d, RECENT_EVENT_LIMIT], |row| {
            Ok(LearningEventSummary {
                skill_name: row.get(0)?,
                action: row.get(1)?,
                status: row.get(2)?,
                message: row.get(3)?,
                summary: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn candidate_skill_names(
    conn: &Connection,
    agent: &str,
    since_7d: &str,
) -> Result<Vec<String>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT candidate_skill_name
         FROM skill_review_reports
         WHERE agent_name=?1
           AND candidate_skill_name IS NOT NULL
           AND created_at >= ?2
         GROUP BY candidate_skill_name
         ORDER BY MAX(created_at) DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![agent, since_7d, CANDIDATE_NAME_LIMIT], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn recent_reports(conn: &Connection, agent: &str) -> Result<Vec<LearningReportSummary>, ReadModelError> {
    let mut stmt = conn.prepare(
        "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                candidate_summary, telegram_notified, created_at
         FROM skill_review_reports
         WHERE agent_name=?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![agent, RECENT_REPORT_LIMIT], report_summary_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn report_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearningReportSummary> {
    Ok(LearningReportSummary {
        id: row.get(0)?,
        status: row.get(1)?,
        confidence: row.get(2)?,
        trigger_kind: row.get(3)?,
        candidate_skill_name: row.get(4)?,
        candidate_summary: row.get(5)?,
        telegram_notified: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}
```

- [ ] **Step 6: Verify overview tests pass**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_overview
```

Expected: pass.

- [ ] **Step 7: Commit Task 3**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/learning.rs
devenv shell -- git commit -m "feat(dashboard): add learning overview read model"
```

## Task 4: Learning Report Detail Read Model

**Files:**
- Modify: `crates/right-dashboard/src/read_model/learning.rs`

- [ ] **Step 1: Add failing report detail tests**

Append these tests to the existing test module in `crates/right-dashboard/src/read_model/learning.rs`:

```rust
    #[test]
    fn learning_report_detail_returns_message_and_execution_snippets() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO conversation_messages (
                id, platform, chat_id, thread_id, message_id, role, content,
                root_session_id, turn_id, routed_to_agent, created_at
             ) VALUES (
                101, 'telegram', 10, 20, 77, 'user',
                'Verify the OAuth callback URL before retrying auth.',
                'session-1', 3, 1, '2026-05-20T10:00:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, tool_name, content_json, content_text, trust_label,
                created_at
             ) VALUES (
                202, 'right', 'session-1', 'inv-1', 3, 9,
                'tool_result', 'shell', '{}', 'callback verified', 'primary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                target_chat_id, target_thread_id, start_ref, end_ref,
                message_refs_json, execution_event_refs_json, selector_model,
                selector_output_json, boundary_rationale, confidence,
                context_incomplete, ready_after, created_at, updated_at
             ) VALUES (
                4, 'right', 'foreground_thread', 'learning_signal', 'inv:inv-1',
                'reviewed', 10, 20, 'msg:101', 'exec:202',
                '[\"msg:101\"]', '[\"exec:202\"]', 'claude-sonnet-4-6',
                '{\"status\":\"selected\"}', 'Selected OAuth setup correction.',
                'high', 0, '2026-05-20T10:01:30Z',
                '2026-05-20T10:00:00Z', '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                root_session_id, chat_id, thread_id, trigger_kind, status,
                confidence, candidate_skill_name, candidate_summary,
                evidence_refs_json, review_output_json, telegram_notified,
                created_at
             ) VALUES (
                9, 'right', 'inv-1', 4, 'session-1', 10, 20,
                'learning_signal', 'create_candidate', 'high',
                'rightx-oauth-debugging', 'Verify OAuth callback setup.',
                '[\"msg:101\",\"exec:202\"]',
                '{\"status\":\"create_candidate\",\"confidence\":\"high\",\"candidate_skill_name\":\"rightx-oauth-debugging\",\"candidate_summary\":\"Verify OAuth callback setup.\",\"evidence_refs\":[\"msg:101\",\"exec:202\"],\"user_notice\":\"notice\"}',
                1, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 9).unwrap().unwrap();

        assert_eq!(detail.report.id, 9);
        assert_eq!(detail.episode.as_ref().unwrap().id, 4);
        assert_eq!(detail.selector.as_ref().unwrap().selected_message_refs, vec!["msg:101"]);
        assert_eq!(detail.evidence.len(), 2);
        assert_eq!(detail.evidence[0].source, "message");
        assert_eq!(detail.evidence[0].text.as_deref(), Some("Verify the OAuth callback URL before retrying auth."));
        assert_eq!(detail.evidence[1].source, "execution_event");
        assert_eq!(detail.evidence[1].event_kind.as_deref(), Some("tool_result"));
        assert_eq!(detail.reviewer.user_notice_present, true);
    }

    #[test]
    fn learning_report_detail_marks_missing_refs_unavailable_and_hides_thinking() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO execution_events (
                id, agent_name, root_session_id, invocation_id, turn_id, seq,
                event_kind, content_json, content_text, trust_label, created_at
             ) VALUES (
                303, 'right', 'session-1', 'inv-2', 5, 1,
                'thinking', '{}', 'private reasoning', 'secondary',
                '2026-05-20T10:01:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO learning_episodes (
                id, agent_name, kind, seed_trigger_kind, seed_ref, status,
                message_refs_json, execution_event_refs_json, selector_output_json,
                ready_after, created_at, updated_at
             ) VALUES (
                5, 'right', 'foreground_thread', 'effort_threshold', 'inv:inv-2',
                'reviewed', '[\"msg:404\"]', '[\"exec:303\"]', '{}',
                '2026-05-20T10:01:30Z', '2026-05-20T10:00:00Z',
                '2026-05-20T10:02:00Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, learning_episode_id,
                trigger_kind, status, confidence, evidence_refs_json,
                review_output_json, telegram_notified, created_at
             ) VALUES (
                10, 'right', 'inv-2', 5, 'effort_threshold',
                'nothing_to_learn', 'medium', '[\"msg:404\",\"exec:303\"]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"medium\",\"evidence_refs\":[\"msg:404\",\"exec:303\"],\"user_notice\":null}',
                0, '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let detail = learning_report_detail(&conn, "right", 10).unwrap().unwrap();

        assert_eq!(detail.evidence.len(), 2);
        assert!(!detail.evidence[0].available);
        assert_eq!(detail.evidence[0].ref_id, "msg:404");
        assert!(!detail.evidence[1].available);
        assert_eq!(detail.evidence[1].ref_id, "exec:303");
        assert_eq!(detail.evidence[1].text, None);
    }

    #[test]
    fn learning_report_detail_errors_on_malformed_report_json() {
        let (_dir, conn) = fixture();
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                11, 'right', 'inv-3', 'effort_threshold', 'failed',
                'low', '[]', '{malformed', '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        assert!(learning_report_detail(&conn, "right", 11).is_err());
    }
```

- [ ] **Step 2: Run the failing detail tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_report_detail
```

Expected: fail because `learning_report_detail()` does not exist.

- [ ] **Step 3: Extend imports**

At the top of `crates/right-dashboard/src/read_model/learning.rs`, extend the DTO import:

```rust
use crate::api_types::{
    LearningCapabilities, LearningEpisodeDetail, LearningEventSummary, LearningEvidenceSnippet,
    LearningFunnel, LearningHealth, LearningLifecycle, LearningOverviewResponse, LearningQuality,
    LearningReportDetailResponse, LearningReportSummary, LearningReviewerDetail,
    LearningSelectorDetail,
};
```

- [ ] **Step 4: Add constants and JSON helpers**

Add near the constants:

```rust
const EVIDENCE_SNIPPET_TEXT_MAX_CHARS: usize = 320;
const EVIDENCE_SNIPPET_LIMIT: usize = 24;
```

Add helper functions near the bottom:

```rust
fn parse_string_array(raw: &str) -> Result<Vec<String>, ReadModelError> {
    Ok(serde_json::from_str(raw)?)
}

fn optional_json(raw: Option<String>) -> Result<Option<serde_json::Value>, ReadModelError> {
    raw.map(|value| serde_json::from_str(&value).map_err(ReadModelError::from))
        .transpose()
}

fn bounded_text(value: String) -> String {
    let mut chars = value.chars();
    let mut out = chars
        .by_ref()
        .take(EVIDENCE_SNIPPET_TEXT_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        out.push_str("... [truncated]");
    }
    out
}

fn parse_ref_id(reference: &str, prefix: &str) -> Option<i64> {
    reference.strip_prefix(prefix)?.parse::<i64>().ok()
}

fn unavailable_snippet(ref_id: String, source: &str) -> LearningEvidenceSnippet {
    LearningEvidenceSnippet {
        ref_id,
        source: source.to_owned(),
        available: false,
        trust_label: None,
        role: None,
        event_kind: None,
        tool_name: None,
        created_at: None,
        text: None,
    }
}
```

- [ ] **Step 5: Implement report detail loading**

Add this public function and support structs:

```rust
struct ReportDetailRow {
    report: LearningReportSummary,
    learning_episode_id: Option<i64>,
    evidence_refs: Vec<String>,
    review_output_json: serde_json::Value,
}

struct EpisodeDetailRow {
    episode: LearningEpisodeDetail,
    selector: LearningSelectorDetail,
    message_refs: Vec<String>,
    execution_event_refs: Vec<String>,
}

pub fn learning_report_detail(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<LearningReportDetailResponse>, ReadModelError> {
    let Some(report_row) = load_report_detail_row(conn, agent, report_id)? else {
        return Ok(None);
    };
    let episode_row = match report_row.learning_episode_id {
        Some(episode_id) => load_episode_detail_row(conn, agent, episode_id)?,
        None => None,
    };
    let allowed_message_refs = episode_row
        .as_ref()
        .map(|row| row.message_refs.as_slice())
        .unwrap_or(&[]);
    let allowed_execution_refs = episode_row
        .as_ref()
        .map(|row| row.execution_event_refs.as_slice())
        .unwrap_or(&[]);
    let evidence_refs = if report_row.evidence_refs.is_empty() {
        episode_row
            .as_ref()
            .map(|row| {
                row.message_refs
                    .iter()
                    .chain(row.execution_event_refs.iter())
                    .take(EVIDENCE_SNIPPET_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        report_row
            .evidence_refs
            .iter()
            .take(EVIDENCE_SNIPPET_LIMIT)
            .cloned()
            .collect()
    };
    let evidence = load_evidence_snippets(
        conn,
        agent,
        &evidence_refs,
        allowed_message_refs,
        allowed_execution_refs,
    )?;
    let reviewer = reviewer_detail(&report_row)?;

    Ok(Some(LearningReportDetailResponse {
        report: report_row.report,
        episode: episode_row.as_ref().map(|row| row.episode.clone()),
        selector: episode_row.map(|row| row.selector),
        evidence,
        reviewer,
    }))
}

fn load_report_detail_row(
    conn: &Connection,
    agent: &str,
    report_id: i64,
) -> Result<Option<ReportDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, status, confidence, trigger_kind, candidate_skill_name,
                    candidate_summary, telegram_notified, created_at,
                    learning_episode_id, evidence_refs_json, review_output_json
             FROM skill_review_reports
             WHERE agent_name=?1 AND id=?2",
            params![agent, report_id],
            |row| {
                Ok((
                    report_summary_from_row(row)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()?;
    row.map(|(report, learning_episode_id, evidence_refs_json, review_output_json)| {
        Ok(ReportDetailRow {
            report,
            learning_episode_id,
            evidence_refs: parse_string_array(&evidence_refs_json)?,
            review_output_json: serde_json::from_str(&review_output_json)?,
        })
    })
    .transpose()
}

fn load_episode_detail_row(
    conn: &Connection,
    agent: &str,
    episode_id: i64,
) -> Result<Option<EpisodeDetailRow>, ReadModelError> {
    let row = conn
        .query_row(
            "SELECT id, kind, seed_trigger_kind, status, start_ref, end_ref,
                    message_refs_json, execution_event_refs_json, selector_model,
                    selector_output_json, boundary_rationale, confidence,
                    context_incomplete
             FROM learning_episodes
             WHERE agent_name=?1 AND id=?2",
            params![agent, episode_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            id,
            kind,
            seed_trigger_kind,
            status,
            start_ref,
            end_ref,
            message_refs_json,
            execution_event_refs_json,
            selector_model,
            selector_output_json,
            boundary_rationale,
            confidence,
            context_incomplete,
        )| {
            let _selector_output = optional_json(selector_output_json)?;
            let message_refs = parse_string_array(&message_refs_json)?;
            let execution_event_refs = parse_string_array(&execution_event_refs_json)?;
            Ok(EpisodeDetailRow {
                episode: LearningEpisodeDetail {
                    id,
                    kind,
                    seed_trigger_kind,
                    status,
                    start_ref,
                    end_ref,
                    boundary_rationale: boundary_rationale.clone(),
                    confidence: confidence.clone(),
                    context_incomplete: context_incomplete != 0,
                },
                selector: LearningSelectorDetail {
                    model: selector_model,
                    boundary_rationale,
                    selected_message_refs: message_refs.clone(),
                    selected_execution_event_refs: execution_event_refs.clone(),
                },
                message_refs,
                execution_event_refs,
            })
        },
    )
    .transpose()
}
```

- [ ] **Step 6: Implement reviewer and evidence helpers**

Add:

```rust
fn reviewer_detail(row: &ReportDetailRow) -> Result<LearningReviewerDetail, ReadModelError> {
    let user_notice_present = row
        .review_output_json
        .get("user_notice")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    Ok(LearningReviewerDetail {
        status: row.report.status.clone(),
        confidence: row.report.confidence.clone(),
        candidate_skill_name: row.report.candidate_skill_name.clone(),
        candidate_summary: row.report.candidate_summary.clone(),
        evidence_refs: row.evidence_refs.clone(),
        user_notice_present,
    })
}

fn load_evidence_snippets(
    conn: &Connection,
    agent: &str,
    refs: &[String],
    allowed_message_refs: &[String],
    allowed_execution_refs: &[String],
) -> Result<Vec<LearningEvidenceSnippet>, ReadModelError> {
    let mut snippets = Vec::with_capacity(refs.len());
    for ref_id in refs {
        if ref_id.starts_with("msg:") {
            if !allowed_message_refs.contains(ref_id) {
                snippets.push(unavailable_snippet(ref_id.clone(), "message"));
                continue;
            }
            snippets.push(load_message_snippet(conn, ref_id)?);
        } else if ref_id.starts_with("exec:") {
            if !allowed_execution_refs.contains(ref_id) {
                snippets.push(unavailable_snippet(ref_id.clone(), "execution_event"));
                continue;
            }
            snippets.push(load_execution_snippet(conn, agent, ref_id)?);
        } else {
            snippets.push(unavailable_snippet(ref_id.clone(), "unknown"));
        }
    }
    Ok(snippets)
}

fn load_message_snippet(
    conn: &Connection,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "msg:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    let row = conn
        .query_row(
            "SELECT role, content, created_at
             FROM conversation_messages
             WHERE id=?1 AND role IN ('user','assistant')",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((role, content, created_at)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "message"));
    };
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "message".to_owned(),
        available: true,
        trust_label: Some("primary".to_owned()),
        role: Some(role),
        event_kind: None,
        tool_name: None,
        created_at: Some(created_at),
        text: Some(bounded_text(content)),
    })
}

fn load_execution_snippet(
    conn: &Connection,
    agent: &str,
    ref_id: &str,
) -> Result<LearningEvidenceSnippet, ReadModelError> {
    let Some(id) = parse_ref_id(ref_id, "exec:") else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    let row = conn
        .query_row(
            "SELECT event_kind, tool_name, content_text, trust_label, created_at
             FROM execution_events
             WHERE agent_name=?1 AND id=?2",
            params![agent, id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((event_kind, tool_name, content_text, trust_label, created_at)) = row else {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    };
    if event_kind == "thinking" || trust_label == "low_trust" {
        return Ok(unavailable_snippet(ref_id.to_owned(), "execution_event"));
    }
    Ok(LearningEvidenceSnippet {
        ref_id: ref_id.to_owned(),
        source: "execution_event".to_owned(),
        available: true,
        trust_label: Some(trust_label),
        role: None,
        event_kind: Some(event_kind),
        tool_name,
        created_at: Some(created_at),
        text: Some(bounded_text(content_text)),
    })
}
```

- [ ] **Step 7: Verify detail tests pass**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning_report_detail
```

Expected: pass.

- [ ] **Step 8: Run all right-dashboard learning tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning
```

Expected: pass.

- [ ] **Step 9: Commit Task 4**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/read_model/learning.rs
devenv shell -- git commit -m "feat(dashboard): add learning report detail read model"
```

## Task 5: Bot Dashboard Routes

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`

- [ ] **Step 1: Add failing route tests**

In `crates/bot/src/telegram/dashboard.rs`, append these tests to the existing test module:

```rust
    async fn get_json(
        path: &str,
        auth: Option<String>,
        agent_dir: std::path::PathBuf,
    ) -> (StatusCode, serde_json::Value) {
        let router = super::build_dashboard_router(test_state(agent_dir));
        let mut builder = Request::builder().uri(path).method("GET");
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, format!("tma {auth}"));
        }
        let response = router
            .oneshot(builder.body(Body::empty()).expect("valid request"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .expect("body bytes");
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json response")
        };
        (status, value)
    }

    #[tokio::test]
    async fn bootstrap_exposes_learning_capabilities() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/bootstrap",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["features"]["learning_metrics"], true);
        assert_eq!(body["features"]["learning_evidence_snippets"], true);
        assert_eq!(body["features"]["learning_commands"], false);
    }

    #[tokio::test]
    async fn learning_overview_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/learning/overview",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["capabilities"]["learning_metrics"], true);
    }

    #[tokio::test]
    async fn learning_overview_rejects_missing_auth() {
        let temp = tempfile::tempdir().expect("tempdir");

        let status = get(
            "/dashboard/alpha/api/v1/learning/overview",
            None,
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn learning_report_detail_returns_not_found_for_unknown_report() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _conn = right_db::open_connection(temp.path(), true).expect("open migrated db");

        let status = get(
            "/dashboard/alpha/api/v1/learning/reports/999",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn learning_report_detail_returns_data_for_authorized_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let conn = right_db::open_connection(temp.path(), true).expect("open migrated db");
        conn.execute(
            "INSERT INTO skill_review_reports (
                id, agent_name, source_invocation_id, trigger_kind, status,
                confidence, evidence_refs_json, review_output_json, created_at
             ) VALUES (
                1, 'alpha', 'inv-1', 'effort_threshold', 'nothing_to_learn',
                'medium', '[]',
                '{\"status\":\"nothing_to_learn\",\"confidence\":\"medium\",\"evidence_refs\":[],\"user_notice\":null}',
                '2026-05-20T11:00:00Z'
             )",
            [],
        )
        .unwrap();

        let (status, body) = get_json(
            "/dashboard/alpha/api/v1/learning/reports/1",
            Some(signed_init_data(42)),
            temp.path().to_path_buf(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["report"]["id"], 1);
        assert_eq!(body["report"]["status"], "nothing_to_learn");
    }
```

- [ ] **Step 2: Run failing route tests**

Run:

```bash
devenv shell -- cargo test -p right-bot learning
```

Expected: fail because routes and bootstrap fields are not fully wired.

- [ ] **Step 3: Import learning read-model items**

In `crates/bot/src/telegram/dashboard.rs`, replace the read-model import with:

```rust
use right_dashboard::read_model::{
    OverviewInput, overview, run_detail,
    learning::{LearningOverviewInput, learning_overview, learning_report_detail},
};
```

- [ ] **Step 4: Mount learning routes**

In `build_dashboard_router`, add routes before the static asset catch-all:

```rust
.route(
    "/dashboard/{agent}/api/v1/learning/overview",
    get(handle_learning_overview),
)
.route(
    "/dashboard/{agent}/api/v1/learning/reports/{report_id}",
    get(handle_learning_report_detail),
)
```

- [ ] **Step 5: Add learning route handlers**

Add below `handle_run_detail`:

```rust
async fn handle_learning_overview(
    AxumPath(agent): AxumPath<String>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    let input = LearningOverviewInput {
        agent: state.agent_name.clone(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        refresh_interval_secs: REFRESH_INTERVAL_SECS,
    };

    match learning_overview(&conn, input) {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, "dashboard learning overview query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_overview_failed",
                Some("failed to read learning overview"),
            )
        }
    }
}

async fn handle_learning_report_detail(
    AxumPath((agent, report_id)): AxumPath<(String, i64)>,
    State(state): State<DashboardState>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authenticate_api(&state, &agent, &headers) {
        return error.into_response();
    }

    let conn = match open_dashboard_read_connection(&state) {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };

    match learning_report_detail(&conn, &state.agent_name, report_id) {
        Ok(Some(response)) => Json(response).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found", Some("learning report not found")),
        Err(error) => {
            tracing::error!(agent = %state.agent_name, report_id, "dashboard learning report query failed: {error:#}");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "learning_report_failed",
                Some("failed to read learning report"),
            )
        }
    }
}
```

- [ ] **Step 6: Verify route tests pass**

Run:

```bash
devenv shell -- cargo test -p right-bot dashboard::
```

Expected: pass.

- [ ] **Step 7: Commit Task 5**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(bot): expose learning dashboard routes"
```

## Task 6: Frontend Types And API Client

**Files:**
- Modify: `crates/right-dashboard/frontend/src/types.ts`
- Modify: `crates/right-dashboard/frontend/src/api.ts`

- [ ] **Step 1: Add TypeScript DTOs**

In `crates/right-dashboard/frontend/src/types.ts`, extend `DashboardFeatures`:

```ts
export interface DashboardFeatures {
  readonly: boolean
  commands_enabled: boolean
  learning_metrics: boolean
  learning_evidence_snippets: boolean
  learning_commands: boolean
}
```

Append these learning types:

```ts
export interface LearningCapabilities {
  learning_metrics: boolean
  learning_evidence_snippets: boolean
  learning_commands: boolean
}

export interface LearningOverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  capabilities: LearningCapabilities
  funnel: LearningFunnel
  quality: LearningQuality
  health: LearningHealth
  lifecycle: LearningLifecycle
  recent_reports: LearningReportSummary[]
}

export interface LearningFunnel {
  signals_accepted_24h: number
  episodes_pending_24h: number
  episodes_selecting_24h: number
  episodes_selected_24h: number
  episodes_reviewing_24h: number
  episodes_reviewed_24h: number
  episodes_no_episode_24h: number
  episodes_failed_24h: number
  reports_total_24h: number
  create_candidates_24h: number
  update_candidates_24h: number
  nothing_to_learn_24h: number
  failed_reviews_24h: number
  foreground_created_or_updated_7d: number
}

export interface LearningQuality {
  candidate_rate: number | null
  nothing_to_learn_rate: number | null
  create_count_24h: number
  update_count_24h: number
  high_confidence_count_24h: number
  medium_confidence_count_24h: number
  low_confidence_count_24h: number
  failed_count_24h: number
}

export interface LearningHealth {
  review_running: boolean
  daily_review_count: number
  daily_limit: number
  creation_review_interval: number
  tool_iters_since_review: number
  turns_since_review: number
  skill_issue_hints_since_review: number
  last_review_status: string | null
  last_review_at: string | null
  possibly_stuck: boolean
}

export interface LearningLifecycle {
  created_7d: number
  updated_7d: number
  failed_or_aborted_7d: number
  recent_successful_events: LearningEventSummary[]
  candidate_skill_names_7d: string[]
}

export interface LearningEventSummary {
  skill_name: string
  action: string
  status: string
  message: string | null
  summary: string | null
  created_at: string
}

export interface LearningReportSummary {
  id: number
  status: string
  confidence: string
  trigger_kind: string
  candidate_skill_name: string | null
  candidate_summary: string | null
  telegram_notified: boolean
  created_at: string
}

export interface LearningReportDetailResponse {
  report: LearningReportSummary
  episode: LearningEpisodeDetail | null
  selector: LearningSelectorDetail | null
  evidence: LearningEvidenceSnippet[]
  reviewer: LearningReviewerDetail
}

export interface LearningEpisodeDetail {
  id: number
  kind: string
  seed_trigger_kind: string
  status: string
  start_ref: string | null
  end_ref: string | null
  boundary_rationale: string | null
  confidence: string | null
  context_incomplete: boolean
}

export interface LearningSelectorDetail {
  model: string | null
  boundary_rationale: string | null
  selected_message_refs: string[]
  selected_execution_event_refs: string[]
}

export interface LearningEvidenceSnippet {
  ref_id: string
  source: string
  available: boolean
  trust_label: string | null
  role: string | null
  event_kind: string | null
  tool_name: string | null
  created_at: string | null
  text: string | null
}

export interface LearningReviewerDetail {
  status: string
  confidence: string
  candidate_skill_name: string | null
  candidate_summary: string | null
  evidence_refs: string[]
  user_notice_present: boolean
}
```

- [ ] **Step 2: Add API client functions**

In `crates/right-dashboard/frontend/src/api.ts`, extend the import:

```ts
import type {
  ApiErrorBody,
  BootstrapResponse,
  LearningOverviewResponse,
  LearningReportDetailResponse,
  OverviewResponse,
  RunDetailResponse,
} from './types'
```

Add:

```ts
export function learningOverview(): Promise<LearningOverviewResponse> {
  return requestJson<LearningOverviewResponse>('api/v1/learning/overview')
}

export function learningReportDetail(reportId: number): Promise<LearningReportDetailResponse> {
  return requestJson<LearningReportDetailResponse>(`api/v1/learning/reports/${encodeURIComponent(String(reportId))}`)
}
```

- [ ] **Step 3: Run frontend typecheck**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

Expected: pass if types are consistent with existing code.

- [ ] **Step 4: Commit Task 6**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts
devenv shell -- git commit -m "feat(dashboard): add learning frontend API types"
```

## Task 7: Frontend Learning View

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue`
- Modify: `crates/right-dashboard/static/dashboard/**`

- [ ] **Step 1: Update imports and state**

In `crates/right-dashboard/frontend/src/App.vue`, replace the API import:

```ts
import { DashboardApiError, bootstrap, learningOverview, learningReportDetail, overview, runDetail } from './api'
```

Replace the type import:

```ts
import type {
  BootstrapResponse,
  CronCard,
  LearningOverviewResponse,
  LearningReportDetailResponse,
  LearningReportSummary,
  OverviewResponse,
  RunDetailResponse,
  RunSummary,
} from './types'
```

Add after `type ConnectionState`:

```ts
type DashboardView = 'cron' | 'learning'
```

Add state after existing refs:

```ts
const activeView = ref<DashboardView>('cron')
const learningData = ref<LearningOverviewResponse | null>(null)
const selectedLearningReport = ref<LearningReportDetailResponse | null>(null)
const selectedLearningReportId = ref<number | null>(null)
const loadingLearningDetail = ref(false)
const learningDetailError = ref<string | null>(null)
```

Add computed properties:

```ts
const learningEnabled = computed(() => bootstrapData.value?.features.learning_metrics === true)
const learningSummary = computed(() => learningData.value)
const learningReports = computed(() => learningData.value?.recent_reports ?? [])
```

- [ ] **Step 2: Update polling logic**

In `refreshOverview()`, after assigning `overviewData.value = data`, add:

```ts
    if (learningEnabled.value) {
      learningData.value = await learningOverview()
    }
```

Add this guard after `bootstrapData.value = await bootstrap()` in `loadInitial()`:

```ts
    if (!bootstrapData.value.features.learning_metrics && activeView.value === 'learning') {
      activeView.value = 'cron'
    }
```

Add helper functions:

```ts
function setView(view: DashboardView): void {
  if (view === 'learning' && !learningEnabled.value) {
    return
  }
  activeView.value = view
}

async function selectLearningReport(report: LearningReportSummary): Promise<void> {
  const reportId = report.id
  selectedLearningReportId.value = reportId
  selectedLearningReport.value = null
  loadingLearningDetail.value = true
  learningDetailError.value = null

  try {
    const detail = await learningReportDetail(reportId)
    if (selectedLearningReportId.value === reportId) {
      selectedLearningReport.value = detail
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    if (selectedLearningReportId.value === reportId) {
      selectedLearningReport.value = null
      learningDetailError.value = error instanceof Error ? error.message : 'Learning report unavailable'
    }
  } finally {
    if (selectedLearningReportId.value === reportId) {
      loadingLearningDetail.value = false
    }
  }
}

function percent(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return 'none'
  }
  return `${Math.round(value * 100)}%`
}
```

- [ ] **Step 3: Add the view switch template**

In the template, after the connection notice section and before the summary grid, add:

```vue
    <nav class="view-tabs" aria-label="Dashboard views">
      <button
        type="button"
        class="tab-button"
        :class="{ active: activeView === 'cron' }"
        @click="setView('cron')"
      >
        Cron
      </button>
      <button
        v-if="learningEnabled"
        type="button"
        class="tab-button"
        :class="{ active: activeView === 'learning' }"
        @click="setView('learning')"
      >
        Learning
      </button>
    </nav>
```

Wrap the existing summary, active strip, and cron content in:

```vue
    <template v-if="activeView === 'cron'">
      <!-- existing summary-grid, active-strip, and content-grid go here -->
    </template>
```

- [ ] **Step 4: Add the Learning view template**

After the Cron template block, add:

```vue
    <template v-else-if="activeView === 'learning'">
      <section class="learning-funnel" aria-label="Learning funnel">
        <article class="summary-card">
          <span>Signals</span>
          <strong>{{ learningSummary?.funnel.signals_accepted_24h ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Episodes</span>
          <strong>{{ learningSummary?.funnel.episodes_reviewed_24h ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Candidates</span>
          <strong>{{ (learningSummary?.funnel.create_candidates_24h ?? 0) + (learningSummary?.funnel.update_candidates_24h ?? 0) }}</strong>
        </article>
        <article class="summary-card">
          <span>Created/updated</span>
          <strong>{{ learningSummary?.funnel.foreground_created_or_updated_7d ?? 0 }}</strong>
        </article>
      </section>

      <section class="learning-grid">
        <section class="cron-list" aria-label="Learning reports">
          <article v-if="learningReports.length === 0" class="empty-panel">
            No learning reports
          </article>

          <button
            v-for="report in learningReports"
            :key="report.id"
            class="learning-report-row"
            :class="{ selected: selectedLearningReportId === report.id }"
            type="button"
            @click="selectLearningReport(report)"
          >
            <span class="run-main">
              <span class="status-dot" :class="statusClass(report.status)"></span>
              <span>{{ report.status }}</span>
              <small>{{ report.confidence }} / {{ report.trigger_kind }}</small>
            </span>
            <span class="report-summary">
              {{ report.candidate_skill_name || report.candidate_summary || 'No candidate' }}
            </span>
          </button>
        </section>

        <aside class="detail-panel" aria-label="Learning metrics">
          <header>
            <p class="eyebrow">Learning quality</p>
            <h2>{{ percent(learningSummary?.quality.candidate_rate) }} candidates</h2>
          </header>

          <dl class="detail-meta">
            <div>
              <dt>Nothing</dt>
              <dd>{{ percent(learningSummary?.quality.nothing_to_learn_rate) }}</dd>
            </div>
            <div>
              <dt>High</dt>
              <dd>{{ learningSummary?.quality.high_confidence_count_24h ?? 0 }}</dd>
            </div>
            <div>
              <dt>Daily</dt>
              <dd>{{ learningSummary?.health.daily_review_count ?? 0 }}/{{ learningSummary?.health.daily_limit ?? 12 }}</dd>
            </div>
            <div>
              <dt>Gate</dt>
              <dd>{{ learningSummary?.health.review_running ? 'running' : 'idle' }}</dd>
            </div>
          </dl>

          <section class="detail-block">
            <h3>Report detail</h3>
            <p v-if="loadingLearningDetail" class="muted-line">Loading learning report</p>
            <p v-else-if="learningDetailError" class="notice inline">{{ learningDetailError }}</p>
            <p v-else-if="!selectedLearningReport" class="muted-line">No report selected</p>

            <template v-if="selectedLearningReport">
              <p>{{ selectedLearningReport.report.candidate_summary || selectedLearningReport.report.status }}</p>
              <p v-if="selectedLearningReport.selector?.boundary_rationale" class="muted-line">
                {{ selectedLearningReport.selector.boundary_rationale }}
              </p>
              <div class="evidence-list">
                <div
                  v-for="snippet in selectedLearningReport.evidence"
                  :key="snippet.ref_id"
                  class="evidence-item"
                >
                  <strong>{{ snippet.ref_id }}</strong>
                  <span>{{ snippet.available ? (snippet.event_kind || snippet.role || snippet.source) : 'unavailable' }}</span>
                  <p>{{ snippet.text || 'Snippet unavailable' }}</p>
                </div>
              </div>
            </template>
          </section>
        </aside>
      </section>
    </template>
```

- [ ] **Step 5: Add styles**

In the `<style scoped>` block, add:

```css
.view-tabs {
  display: flex;
  gap: 6px;
  margin-bottom: 10px;
}

.tab-button {
  min-height: 32px;
  padding: 5px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
}

.tab-button.active {
  border-color: var(--tg-theme-button_color, #2481cc);
  color: var(--tg-theme-button_color, #2481cc);
  font-weight: 700;
}

.learning-funnel {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  margin-bottom: 10px;
}

.learning-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(320px, 0.7fr);
  gap: 8px;
  align-items: start;
}

.learning-report-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 5px;
  width: 100%;
  min-height: 58px;
  padding: 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  text-align: left;
}

.learning-report-row.selected {
  border-color: var(--tg-theme-button_color, #2481cc);
  box-shadow: inset 0 0 0 1px var(--tg-theme-button_color, #2481cc);
}

.report-summary {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.78rem;
  overflow-wrap: anywhere;
}

.evidence-list {
  display: grid;
  gap: 7px;
}

.evidence-item {
  display: grid;
  gap: 3px;
  padding: 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
}

.evidence-item strong,
.evidence-item span {
  font-size: 0.74rem;
}

.evidence-item span {
  color: var(--tg-theme-hint-color, #6b7b88);
}

.evidence-item p {
  font-size: 0.78rem;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

@media (max-width: 820px) {
  .learning-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}

@media (max-width: 560px) {
  .learning-funnel {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
```

- [ ] **Step 6: Typecheck and build frontend**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: both pass. `npm run build` updates `crates/right-dashboard/static/dashboard/`.

- [ ] **Step 7: Verify static assets changed only from build**

Run:

```bash
devenv shell -- git status --short crates/right-dashboard/frontend crates/right-dashboard/static/dashboard
```

Expected: `App.vue`, `types.ts`, `api.ts`, and built static assets are modified; no `node_modules`.

- [ ] **Step 8: Commit Task 7**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "feat(dashboard): add learning metrics view"
```

## Task 8: Architecture Docs

**Files:**
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update module docs**

In `docs/architecture/modules.md`, update the `right-dashboard` section to:

```markdown
### right-dashboard

- `auth.rs` — Telegram Mini App `initData` validation and allowlist authorization helpers.
- `api_types.rs` — dashboard API DTOs and error response bodies, including cron and learned-skill metrics DTOs.
- `read_model.rs` — read-only SQLite projections for cron dashboard views and the public entry point for read-model helpers.
- `read_model/learning.rs` — read-only learned-skill dashboard projections over `skill_nudge_signals`, `learning_episodes`, `skill_review_reports`, `skill_learning_events`, `execution_events`, and `conversation_messages`.
- `assets.rs` — embedded static dashboard asset lookup and content types.
- `frontend/` — Vue/Vite source for the Mini App dashboard.
- `static/dashboard/` — checked-in built dashboard assets served by the bot.
```

- [ ] **Step 2: Update lifecycle docs**

In `docs/architecture/lifecycle.md`, update the dashboard bullet under `right bot --agent <name>` to:

```markdown
  ├─ Start bot-owned UDS server with OAuth callback, progress, healthz,
  │   dashboard, and nested Telegram webhook routes; dashboard serves
  │   `/dashboard/<agent>/` static assets plus explicit read-only v1 API
  │   endpoints for cron overview/run detail and learned-skill metrics/report
  │   detail
```

- [ ] **Step 3: Commit Task 8**

Run:

```bash
devenv shell -- git add docs/architecture/modules.md docs/architecture/lifecycle.md
devenv shell -- git commit -m "docs(architecture): document dashboard learning metrics"
```

## Task 9: Final Verification

**Files:**
- Read: full worktree

- [ ] **Step 1: Run targeted Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard::
```

Expected: pass.

- [ ] **Step 2: Run frontend verification**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: pass. If build changes `crates/right-dashboard/static/dashboard/`, commit the regenerated assets:

```bash
devenv shell -- git add crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "build(dashboard): refresh learning static assets"
```

- [ ] **Step 3: Run full workspace verification**

Run:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

Expected: pass.

- [ ] **Step 4: Confirm final status**

Run:

```bash
devenv shell -- git status --short --branch
```

Expected: clean branch.

- [ ] **Step 5: Prepare completion**

Use `superpowers:requesting-code-review` before merging or pushing. Do not merge this branch until the code review loop is clean and the full verification from Step 3 has passed after the last code change.
