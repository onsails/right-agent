# Background Learned-Skill Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a report-only background reviewer that inspects completed foreground turns for reusable learned-skill candidates and records review telemetry without mutating skill files.

**Architecture:** Extend the learned-skill DB/domain model with review reports and review gates, add pure bot-side review helpers for structured output and bounded bundles, then wire a post-foreground scheduler that starts a separate background Claude Code invocation only when gates pass. Stage 2 disallows mutation tools and records reports; it sends Telegram only for high-confidence candidates.

**Tech Stack:** Rust 2024, rusqlite migrations via `right-db`, existing `right-agent::learned_skills` helpers, `right-bot` Telegram worker, Claude Code `--output-format json` structured output, OpenShell sandbox read helpers.

---

## Scope

This plan implements Stage 2 from:

`docs/superpowers/specs/2026-05-18-background-learned-skill-review-design.md`

Included:

- `skill_review_reports` persistence.
- Review gate state in `skill_nudge_state`.
- Report-only review output schema and parser.
- Bounded review bundle/prompt helpers.
- `rightx-*` skill index collection for host and sandboxed agents.
- Background review invocation after foreground completion.
- High-confidence Telegram candidate notice.
- Tests for DB, gates, bundle helpers, tool boundary, and worker integration seams.

Excluded:

- Skill file create/update/delete/archive.
- Curator stale/archive/pin lifecycle.
- Approval UI for drafts.
- GEPA/offline optimization.

## File Map

- Create `crates/right-db/src/sql/v22_skill_review_reports.sql`: review reports table and nudge-state columns.
- Modify `crates/right-db/src/migrations.rs`: register v22 migration, expose latest schema version, and add migration tests.
- Modify `crates/right-db/tests/smoke.rs`: assert against latest schema version instead of a hard-coded value.
- Modify `crates/right-agent/src/doctor.rs`: use the latest schema version exported by `right-db`.
- Modify `crates/right-agent/src/learned_skills.rs`: review enums, report persistence, gate helpers, and tests.
- Create `crates/bot/src/learning_review.rs`: review output schema, prompt/bundle structs, skill-index parsing, tool boundary helpers, and tests.
- Modify `crates/bot/src/lib.rs`: expose `learning_review` module internally.
- Modify `crates/bot/src/telegram/worker.rs`: collect review input, schedule background review, record reports, send high-confidence notice.
- Modify `crates/bot/src/cc/invocation.rs`: add background-review mutation-tool deny helper and tests.
- Modify `docs/architecture/sessions.md`: document background learned-skill review invocation.
- Modify `docs/architecture/mcp.md`: document that Stage 2 background review is report-only and denies learning tools.
- Modify `PROMPT_SYSTEM.md`: keep learned-skill behavior in sync.

## Verification Cadence

Use targeted tests while implementing:

```bash
devenv shell -- cargo test -p right-db skill_review
devenv shell -- cargo test -p right-agent learned_skills::tests::review
devenv shell -- cargo test -p right-bot learning_review
```

Final verification:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

---

### Task 1: Database Schema

**Files:**
- Create: `crates/right-db/src/sql/v22_skill_review_reports.sql`
- Modify: `crates/right-db/src/migrations.rs`
- Modify: `crates/right-db/tests/smoke.rs`
- Modify: `crates/right-agent/src/doctor.rs`

- [ ] **Step 1: Write failing migration tests**

In `crates/right-db/src/migrations.rs`, add these tests inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn skill_review_reports_migration_creates_report_table() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='skill_review_reports'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 1, "skill_review_reports table must exist");

    for column in [
        "agent_name",
        "source_invocation_id",
        "trigger_kind",
        "status",
        "confidence",
        "candidate_skill_name",
        "review_output_json",
        "telegram_notified",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('skill_review_reports') WHERE name = ?1",
                [column],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{column} column must exist");
    }
}

#[test]
fn skill_nudge_state_has_review_gate_defaults() {
    let mut conn = Connection::open_in_memory().unwrap();
    MIGRATIONS.to_latest(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO skill_nudge_state (agent_name) VALUES ('right')",
        [],
    )
    .unwrap();

    let row: (i64, i64, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT creation_review_interval, daily_review_count, daily_review_date, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, (15, 0, None, None));
}
```

- [ ] **Step 2: Run migration tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-db skill_review
```

Expected: FAIL because v22 schema is not registered and `skill_review_reports` does not exist.

- [ ] **Step 3: Add v22 SQL migration**

Create `crates/right-db/src/sql/v22_skill_review_reports.sql`:

```sql
CREATE TABLE IF NOT EXISTS skill_review_reports (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  source_invocation_id TEXT NOT NULL,
  root_session_id TEXT,
  chat_id INTEGER,
  thread_id INTEGER,
  trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('learning_signal', 'skill_issue_signal', 'effort_threshold')),
  status TEXT NOT NULL CHECK (status IN ('nothing_to_learn', 'create_candidate', 'update_candidate', 'failed')),
  confidence TEXT NOT NULL CHECK (confidence IN ('low', 'medium', 'high')),
  candidate_skill_name TEXT,
  candidate_summary TEXT,
  evidence_refs_json TEXT NOT NULL DEFAULT '[]',
  review_output_json TEXT NOT NULL,
  telegram_notified INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_review_reports_agent_created
  ON skill_review_reports(agent_name, created_at);

CREATE INDEX IF NOT EXISTS idx_skill_review_reports_invocation
  ON skill_review_reports(source_invocation_id);

ALTER TABLE skill_nudge_state
  ADD COLUMN creation_review_interval INTEGER NOT NULL DEFAULT 15;

ALTER TABLE skill_nudge_state
  ADD COLUMN daily_review_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE skill_nudge_state
  ADD COLUMN daily_review_date TEXT;

ALTER TABLE skill_nudge_state
  ADD COLUMN last_review_status TEXT;
```

- [ ] **Step 4: Register v22 migration and latest schema constant**

In `crates/right-db/src/migrations.rs`, add this constant after the existing `V21_SCHEMA`:

```rust
const V22_SCHEMA: &str = include_str!("sql/v22_skill_review_reports.sql");

pub const LATEST_SCHEMA_VERSION: u32 = 22;
```

Append the migration after `M::up(V21_SCHEMA),`:

```rust
M::up(V22_SCHEMA),
```

- [ ] **Step 5: Replace stale hard-coded schema version checks**

In `crates/right-db/tests/smoke.rs`, change the two `21` assertions in `open_connection_applies_migrations` and `open_connection_without_migration_preserves_existing_schema` to:

```rust
right_db::migrations::LATEST_SCHEMA_VERSION as i64
```

In `crates/right-agent/src/doctor.rs`, replace:

```rust
let expected: u32 = 20;
```

with:

```rust
let expected: u32 = right_db::migrations::LATEST_SCHEMA_VERSION;
```

- [ ] **Step 6: Run migration and baseline schema tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-db skill_review
devenv shell -- cargo test -p right-db open_connection_applies_migrations
devenv shell -- cargo test -p right-agent check_memory_passes_on_empty_queue
```

Expected: PASS.

- [ ] **Step 7: Commit database slice**

Run:

```bash
devenv shell -- git add crates/right-db/src/sql/v22_skill_review_reports.sql crates/right-db/src/migrations.rs crates/right-db/tests/smoke.rs crates/right-agent/src/doctor.rs
devenv shell -- git commit -m "feat(db): add learned skill review reports"
```

Expected: commit succeeds.

---

### Task 2: Domain Types, Report Persistence, And Gates

**Files:**
- Modify: `crates/right-agent/src/learned_skills.rs`

- [ ] **Step 1: Write failing report persistence test**

In `crates/right-agent/src/learned_skills.rs`, add this test inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn review_report_persistence_round_trips_candidate() {
    let conn = conn();
    let report = SkillReviewReport {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        chat_id: Some(100),
        thread_id: Some(200),
        trigger_kind: ReviewTriggerKind::LearningSignal,
        status: ReviewStatus::CreateCandidate,
        confidence: ReviewConfidence::High,
        candidate_skill_name: Some("rightx-oauth-debugging".to_owned()),
        candidate_summary: Some("OAuth MCP setup needs callback URL verification.".to_owned()),
        evidence_refs: vec!["event-1".to_owned(), "event-2".to_owned()],
        review_output_json: serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "rightx-oauth-debugging"
        }),
        telegram_notified: true,
    };

    insert_skill_review_report(&conn, &report).unwrap();

    let row: (String, String, String, String, String, i64) = conn
        .query_row(
            "SELECT trigger_kind, status, confidence, candidate_skill_name, evidence_refs_json, telegram_notified \
             FROM skill_review_reports WHERE source_invocation_id='inv-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "learning_signal");
    assert_eq!(row.1, "create_candidate");
    assert_eq!(row.2, "high");
    assert_eq!(row.3, "rightx-oauth-debugging");
    assert_eq!(row.4, r#"["event-1","event-2"]"#);
    assert_eq!(row.5, 1);
}
```

- [ ] **Step 2: Write failing gate tests**

In the same test module, add:

```rust
#[test]
fn review_gate_accepts_signal_and_effort_threshold() {
    let conn = conn();
    ensure_nudge_state(&conn, "right").unwrap();

    let signal_decision = review_gate_decision(
        &conn,
        "right",
        ReviewGateInput {
            has_signal: true,
            today: "2026-05-18",
            cooldown_elapsed: true,
            daily_limit: 12,
        },
    )
    .unwrap();
    assert_eq!(
        signal_decision,
        ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal)
    );

    conn.execute(
        "UPDATE skill_nudge_state SET tool_iters_since_review = 15 WHERE agent_name='right'",
        [],
    )
    .unwrap();
    let effort_decision = review_gate_decision(
        &conn,
        "right",
        ReviewGateInput {
            has_signal: false,
            today: "2026-05-18",
            cooldown_elapsed: true,
            daily_limit: 12,
        },
    )
    .unwrap();
    assert_eq!(
        effort_decision,
        ReviewGateDecision::Start(ReviewTriggerKind::EffortThreshold)
    );
}

#[test]
fn review_gate_blocks_running_cooldown_and_daily_limit() {
    let conn = conn();
    ensure_nudge_state(&conn, "right").unwrap();

    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 1 WHERE agent_name='right'",
        [],
    )
    .unwrap();
    let running = review_gate_decision(
        &conn,
        "right",
        ReviewGateInput {
            has_signal: true,
            today: "2026-05-18",
            cooldown_elapsed: true,
            daily_limit: 12,
        },
    )
    .unwrap();
    assert_eq!(running, ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning));

    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 0, daily_review_count = 12, daily_review_date = '2026-05-18' \
         WHERE agent_name='right'",
        [],
    )
    .unwrap();
    let limited = review_gate_decision(
        &conn,
        "right",
        ReviewGateInput {
            has_signal: true,
            today: "2026-05-18",
            cooldown_elapsed: true,
            daily_limit: 12,
        },
    )
    .unwrap();
    assert_eq!(limited, ReviewGateDecision::Skip(ReviewSkipReason::DailyLimit));

    conn.execute(
        "UPDATE skill_nudge_state SET daily_review_count = 0 WHERE agent_name='right'",
        [],
    )
    .unwrap();
    let cooldown = review_gate_decision(
        &conn,
        "right",
        ReviewGateInput {
            has_signal: true,
            today: "2026-05-18",
            cooldown_elapsed: false,
            daily_limit: 12,
        },
    )
    .unwrap();
    assert_eq!(cooldown, ReviewGateDecision::Skip(ReviewSkipReason::Cooldown));
}

#[test]
fn review_start_and_finish_update_nudge_state() {
    let conn = conn();
    ensure_nudge_state(&conn, "right").unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET tool_iters_since_review = 19, turns_since_review = 3 WHERE agent_name='right'",
        [],
    )
    .unwrap();

    mark_review_started(&conn, "right", "2026-05-18").unwrap();
    let running: i64 = conn
        .query_row(
            "SELECT review_running FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(running, 1);

    mark_review_finished(&conn, "right", ReviewStatus::NothingToLearn, true).unwrap();
    let row: (i64, i64, i64, String) = conn
        .query_row(
            "SELECT review_running, tool_iters_since_review, turns_since_review, last_review_status \
             FROM skill_nudge_state WHERE agent_name='right'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row, (0, 0, 0, "nothing_to_learn".to_owned()));
}
```

- [ ] **Step 3: Run domain tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-agent review_
```

Expected: FAIL because the review types/functions do not exist.

- [ ] **Step 4: Add review enums and structs**

In `crates/right-agent/src/learned_skills.rs`, add after `NudgeSignalRecord`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewTriggerKind {
    LearningSignal,
    SkillIssueSignal,
    EffortThreshold,
}

impl ReviewTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LearningSignal => "learning_signal",
            Self::SkillIssueSignal => "skill_issue_signal",
            Self::EffortThreshold => "effort_threshold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    NothingToLearn,
    CreateCandidate,
    UpdateCandidate,
    Failed,
}

impl ReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NothingToLearn => "nothing_to_learn",
            Self::CreateCandidate => "create_candidate",
            Self::UpdateCandidate => "update_candidate",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewConfidence {
    Low,
    Medium,
    High,
}

impl ReviewConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillReviewReport {
    pub agent_name: String,
    pub source_invocation_id: String,
    pub root_session_id: Option<String>,
    pub chat_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub trigger_kind: ReviewTriggerKind,
    pub status: ReviewStatus,
    pub confidence: ReviewConfidence,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub evidence_refs: Vec<String>,
    pub review_output_json: serde_json::Value,
    pub telegram_notified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewGateInput<'a> {
    pub has_signal: bool,
    pub today: &'a str,
    pub cooldown_elapsed: bool,
    pub daily_limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewSkipReason {
    AlreadyRunning,
    Cooldown,
    DailyLimit,
    BelowThreshold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewGateDecision {
    Start(ReviewTriggerKind),
    Skip(ReviewSkipReason),
}
```

- [ ] **Step 5: Add report persistence and gate helpers**

In `crates/right-agent/src/learned_skills.rs`, add after `record_nudge_signal`:

```rust
pub fn insert_skill_review_report(
    conn: &rusqlite::Connection,
    report: &SkillReviewReport,
) -> Result<(), rusqlite::Error> {
    let evidence_refs_json = serde_json::to_string(&report.evidence_refs)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let review_output_json = serde_json::to_string(&report.review_output_json)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO skill_review_reports \
         (agent_name, source_invocation_id, root_session_id, chat_id, thread_id, trigger_kind, status, confidence, candidate_skill_name, candidate_summary, evidence_refs_json, review_output_json, telegram_notified) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            report.agent_name,
            report.source_invocation_id,
            report.root_session_id,
            report.chat_id,
            report.thread_id,
            report.trigger_kind.as_str(),
            report.status.as_str(),
            report.confidence.as_str(),
            report.candidate_skill_name,
            report.candidate_summary,
            evidence_refs_json,
            review_output_json,
            if report.telegram_notified { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

pub fn review_gate_decision(
    conn: &rusqlite::Connection,
    agent_name: &str,
    input: ReviewGateInput<'_>,
) -> Result<ReviewGateDecision, rusqlite::Error> {
    ensure_nudge_state(conn, agent_name)?;
    let row: (i64, i64, i64, Option<String>, i64) = conn.query_row(
        "SELECT review_running, daily_review_count, tool_iters_since_review, daily_review_date, creation_review_interval \
         FROM skill_nudge_state WHERE agent_name = ?1",
        [agent_name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;

    let (review_running, mut daily_count, tool_iters, daily_date, interval) = row;
    if review_running != 0 {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::AlreadyRunning));
    }
    if !input.cooldown_elapsed {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::Cooldown));
    }
    if daily_date.as_deref() != Some(input.today) {
        daily_count = 0;
    }
    if daily_count >= input.daily_limit {
        return Ok(ReviewGateDecision::Skip(ReviewSkipReason::DailyLimit));
    }
    if input.has_signal {
        return Ok(ReviewGateDecision::Start(ReviewTriggerKind::LearningSignal));
    }
    if tool_iters >= interval {
        return Ok(ReviewGateDecision::Start(ReviewTriggerKind::EffortThreshold));
    }
    Ok(ReviewGateDecision::Skip(ReviewSkipReason::BelowThreshold))
}

pub fn mark_review_started(
    conn: &rusqlite::Connection,
    agent_name: &str,
    today: &str,
) -> Result<(), rusqlite::Error> {
    ensure_nudge_state(conn, agent_name)?;
    conn.execute(
        "UPDATE skill_nudge_state \
         SET review_running = 1, \
             daily_review_date = ?2, \
             daily_review_count = CASE WHEN daily_review_date = ?2 THEN daily_review_count + 1 ELSE 1 END \
         WHERE agent_name = ?1",
        rusqlite::params![agent_name, today],
    )?;
    Ok(())
}

pub fn mark_review_finished(
    conn: &rusqlite::Connection,
    agent_name: &str,
    status: ReviewStatus,
    reset_counters: bool,
) -> Result<(), rusqlite::Error> {
    ensure_nudge_state(conn, agent_name)?;
    if reset_counters {
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 0, tool_iters_since_review = 0, turns_since_review = 0, skill_issue_hints_since_review = 0, last_review_at = strftime('%Y-%m-%dT%H:%M:%SZ','now'), last_review_status = ?2 \
             WHERE agent_name = ?1",
            rusqlite::params![agent_name, status.as_str()],
        )?;
    } else {
        conn.execute(
            "UPDATE skill_nudge_state \
             SET review_running = 0, last_review_at = strftime('%Y-%m-%dT%H:%M:%SZ','now'), last_review_status = ?2 \
             WHERE agent_name = ?1",
            rusqlite::params![agent_name, status.as_str()],
        )?;
    }
    Ok(())
}
```

- [ ] **Step 6: Run domain tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-agent review_
```

Expected: PASS.

- [ ] **Step 7: Commit domain slice**

Run:

```bash
devenv shell -- git add crates/right-agent/src/learned_skills.rs
devenv shell -- git commit -m "feat(agent): add learned skill review gates"
```

Expected: commit succeeds.

---

### Task 3: Review Output, Prompt, And Tool Boundary Helpers

**Files:**
- Create: `crates/bot/src/learning_review.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/cc/invocation.rs`

- [ ] **Step 1: Write failing review helper tests**

Create `crates/bot/src/learning_review.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_high_confidence_create_candidate() {
        let value = serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "rightx-oauth-debugging",
            "candidate_summary": "Capture verified OAuth callback setup.",
            "evidence_refs": ["event-1", "event-2"],
            "user_notice": "I found a reusable workflow candidate for OAuth MCP setup."
        });

        let output = ReviewOutput::parse(value).unwrap();
        assert_eq!(output.status, ReviewOutputStatus::CreateCandidate);
        assert_eq!(output.confidence, ReviewOutputConfidence::High);
        assert!(output.should_notify_user());
    }

    #[test]
    fn low_confidence_candidate_does_not_notify() {
        let value = serde_json::json!({
            "status": "update_candidate",
            "confidence": "medium",
            "candidate_skill_name": "rightx-oauth-debugging",
            "candidate_summary": "Maybe add token refresh note.",
            "evidence_refs": ["event-1"],
            "user_notice": "This should not be sent."
        });

        let output = ReviewOutput::parse(value).unwrap();
        assert!(!output.should_notify_user());
    }

    #[test]
    fn rejects_non_rightx_candidate_name() {
        let value = serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "oauth-debugging",
            "candidate_summary": "Capture verified OAuth callback setup.",
            "evidence_refs": ["event-1"],
            "user_notice": "Candidate."
        });

        let err = ReviewOutput::parse(value).unwrap_err();
        assert!(err.contains("rightx-"), "{err}");
    }

    #[test]
    fn nothing_to_learn_accepts_empty_candidate_fields() {
        let value = serde_json::json!({
            "status": "nothing_to_learn",
            "confidence": "low",
            "candidate_skill_name": null,
            "candidate_summary": null,
            "evidence_refs": [],
            "user_notice": null
        });

        let output = ReviewOutput::parse(value).unwrap();
        assert_eq!(output.status, ReviewOutputStatus::NothingToLearn);
        assert!(!output.should_notify_user());
    }

    #[test]
    fn review_prompt_says_report_only_and_nothing_to_learn_is_normal() {
        let bundle = ReviewBundle {
            agent_name: "right".to_owned(),
            source_invocation_id: "inv-1".to_owned(),
            root_session_id: Some("session-1".to_owned()),
            trigger_kind: "effort_threshold".to_owned(),
            accepted_signal_json: None,
            tool_iters_since_review: 15,
            turns_since_review: 3,
            skill_issue_hints_since_review: 0,
            event_timeline: vec!["event-1 user asked for OAuth setup".to_owned()],
            learning_events: vec!["start create rightx-oauth-debugging".to_owned()],
            learned_skills: vec![LearnedSkillSummary {
                name: "rightx-oauth-debugging".to_owned(),
                excerpt: "description: Use for OAuth MCP setup".to_owned(),
            }],
        };
        let prompt = build_review_prompt(&bundle);
        assert!(prompt.contains("Report-only"));
        assert!(prompt.contains("Do not write files"));
        assert!(prompt.contains("nothing_to_learn is normal"));
        assert!(prompt.contains("rightx-oauth-debugging"));
    }

    #[test]
    fn stream_event_timeline_is_stable_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let agent_dir = temp.path().join("agents/right");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let log_path = review_stream_log_path(&agent_dir, "session-1");
        std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        std::fs::write(
            &log_path,
            concat!(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"checked OAuth callback settings"}]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"right mcp list --agent right"}}]}}"#,
                "\n",
                r#"{"type":"result","num_turns":3,"total_cost_usd":0.01,"session_id":"session-1"}"#,
                "\n"
            ),
        )
        .unwrap();

        let timeline = collect_stream_event_timeline(&agent_dir, "session-1", 8).unwrap();
        assert_eq!(timeline.len(), 3);
        assert!(timeline[0].starts_with("event-1 assistant_text: checked OAuth"));
        assert!(timeline[1].contains("tool_use Bash"));
        assert!(timeline[2].contains("result"));
    }
}
```

- [ ] **Step 2: Add module declaration and verify failure**

In `crates/bot/src/lib.rs`, add:

```rust
pub(crate) mod learning_review;
```

Run:

```bash
devenv shell -- cargo test -p right-bot learning_review
```

Expected: FAIL because helper types do not exist.

- [ ] **Step 3: Implement review output and prompt helpers**

Replace `crates/bot/src/learning_review.rs` with:

```rust
use right_mcp::LEARNED_SKILL_PREFIX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewOutputStatus {
    NothingToLearn,
    CreateCandidate,
    UpdateCandidate,
    Failed,
}

impl ReviewOutputStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "nothing_to_learn" => Some(Self::NothingToLearn),
            "create_candidate" => Some(Self::CreateCandidate),
            "update_candidate" => Some(Self::UpdateCandidate),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub(crate) fn as_domain(self) -> right_agent::learned_skills::ReviewStatus {
        match self {
            Self::NothingToLearn => right_agent::learned_skills::ReviewStatus::NothingToLearn,
            Self::CreateCandidate => right_agent::learned_skills::ReviewStatus::CreateCandidate,
            Self::UpdateCandidate => right_agent::learned_skills::ReviewStatus::UpdateCandidate,
            Self::Failed => right_agent::learned_skills::ReviewStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewOutputConfidence {
    Low,
    Medium,
    High,
}

impl ReviewOutputConfidence {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    pub(crate) fn as_domain(self) -> right_agent::learned_skills::ReviewConfidence {
        match self {
            Self::Low => right_agent::learned_skills::ReviewConfidence::Low,
            Self::Medium => right_agent::learned_skills::ReviewConfidence::Medium,
            Self::High => right_agent::learned_skills::ReviewConfidence::High,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewOutput {
    pub(crate) status: ReviewOutputStatus,
    pub(crate) confidence: ReviewOutputConfidence,
    pub(crate) candidate_skill_name: Option<String>,
    pub(crate) candidate_summary: Option<String>,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) user_notice: Option<String>,
    pub(crate) raw: serde_json::Value,
}

impl ReviewOutput {
    pub(crate) fn parse(raw: serde_json::Value) -> Result<Self, String> {
        let status = raw
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(ReviewOutputStatus::parse)
            .ok_or_else(|| "review output status is invalid".to_owned())?;
        let confidence = raw
            .get("confidence")
            .and_then(|v| v.as_str())
            .and_then(ReviewOutputConfidence::parse)
            .ok_or_else(|| "review output confidence is invalid".to_owned())?;
        let candidate_skill_name = raw
            .get("candidate_skill_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        if let Some(name) = &candidate_skill_name
            && !name.starts_with(LEARNED_SKILL_PREFIX)
        {
            return Err(format!("candidate_skill_name must start with {LEARNED_SKILL_PREFIX}"));
        }
        let evidence_refs = raw
            .get("evidence_refs")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "review output evidence_refs must be an array".to_owned())?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| "review output evidence_refs must contain non-empty strings".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if matches!(
            status,
            ReviewOutputStatus::CreateCandidate | ReviewOutputStatus::UpdateCandidate
        ) && evidence_refs.is_empty()
        {
            return Err("candidate review output requires evidence_refs".to_owned());
        }
        let candidate_summary = raw
            .get("candidate_summary")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        let user_notice = raw
            .get("user_notice")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        Ok(Self {
            status,
            confidence,
            candidate_skill_name,
            candidate_summary,
            evidence_refs,
            user_notice,
            raw,
        })
    }

    pub(crate) fn should_notify_user(&self) -> bool {
        matches!(
            self.status,
            ReviewOutputStatus::CreateCandidate | ReviewOutputStatus::UpdateCandidate
        ) && self.confidence == ReviewOutputConfidence::High
            && self.user_notice.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LearnedSkillSummary {
    pub(crate) name: String,
    pub(crate) excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewBundle {
    pub(crate) agent_name: String,
    pub(crate) source_invocation_id: String,
    pub(crate) root_session_id: Option<String>,
    pub(crate) trigger_kind: String,
    pub(crate) accepted_signal_json: Option<String>,
    pub(crate) tool_iters_since_review: i64,
    pub(crate) turns_since_review: i64,
    pub(crate) skill_issue_hints_since_review: i64,
    pub(crate) event_timeline: Vec<String>,
    pub(crate) learning_events: Vec<String>,
    pub(crate) learned_skills: Vec<LearnedSkillSummary>,
}

pub(crate) fn build_review_prompt(bundle: &ReviewBundle) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Background Learned-Skill Review\n\n");
    prompt.push_str("Report-only review. Do not write files. Do not call learning tools. Do not ask the user questions. nothing_to_learn is normal when evidence is weak.\n\n");
    prompt.push_str(&format!("agent_name: {}\n", bundle.agent_name));
    prompt.push_str(&format!("source_invocation_id: {}\n", bundle.source_invocation_id));
    if let Some(root_session_id) = &bundle.root_session_id {
        prompt.push_str(&format!("root_session_id: {}\n", root_session_id));
    }
    prompt.push_str(&format!("trigger_kind: {}\n", bundle.trigger_kind));
    prompt.push_str(&format!("tool_iters_since_review: {}\n", bundle.tool_iters_since_review));
    prompt.push_str(&format!("turns_since_review: {}\n", bundle.turns_since_review));
    prompt.push_str(&format!(
        "skill_issue_hints_since_review: {}\n\n",
        bundle.skill_issue_hints_since_review
    ));
    if let Some(signal) = &bundle.accepted_signal_json {
        prompt.push_str("accepted_signal_json:\n");
        prompt.push_str(signal);
        prompt.push_str("\n\n");
    }
    prompt.push_str("event_timeline:\n");
    for event in &bundle.event_timeline {
        prompt.push_str("- ");
        prompt.push_str(event);
        prompt.push('\n');
    }
    prompt.push_str("\nlearning_events:\n");
    for event in &bundle.learning_events {
        prompt.push_str("- ");
        prompt.push_str(event);
        prompt.push('\n');
    }
    prompt.push_str("\nrightx_skill_index:\n");
    for skill in &bundle.learned_skills {
        prompt.push_str("- ");
        prompt.push_str(&skill.name);
        prompt.push_str(": ");
        prompt.push_str(&skill.excerpt.replace('\n', " "));
        prompt.push('\n');
    }
    prompt.push_str("\nReturn JSON matching the configured schema with status, confidence, candidate_skill_name, candidate_summary, evidence_refs, and user_notice.\n");
    prompt
}

pub(crate) fn review_stream_log_path(
    agent_dir: &std::path::Path,
    root_session_id: &str,
) -> std::path::PathBuf {
    agent_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(agent_dir)
        .join("logs")
        .join("streams")
        .join(format!("{root_session_id}.ndjson"))
}

pub(crate) fn collect_stream_event_timeline(
    agent_dir: &std::path::Path,
    root_session_id: &str,
    max_events: usize,
) -> std::io::Result<Vec<String>> {
    let path = review_stream_log_path(agent_dir, root_session_id);
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for line in std::io::BufRead::lines(reader) {
        if out.len() >= max_events {
            break;
        }
        let line = line?;
        let summary = match crate::cc::stream::parse_stream_event(&line) {
            crate::cc::stream::StreamEvent::Text(text) => {
                Some(format!("assistant_text: {}", bounded_event_text(&text)))
            }
            crate::cc::stream::StreamEvent::ToolUse { tool, input_summary } => {
                Some(format!("tool_use {}: {}", tool, bounded_event_text(&input_summary)))
            }
            crate::cc::stream::StreamEvent::Result(_) => Some("result: foreground invocation completed".to_owned()),
            crate::cc::stream::StreamEvent::Thinking | crate::cc::stream::StreamEvent::Other => None,
        };
        if let Some(summary) = summary {
            out.push(format!("event-{} {}", out.len() + 1, summary));
        }
    }
    Ok(out)
}

fn bounded_event_text(value: &str) -> String {
    const MAX_CHARS: usize = 280;
    let mut out = value.chars().take(MAX_CHARS).collect::<String>();
    if value.chars().count() > MAX_CHARS {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests;
```

Then move the tests from Step 1 into `crates/bot/src/learning_review_tests.rs` and add at the bottom of `learning_review.rs`:

```rust
#[cfg(test)]
#[path = "learning_review_tests.rs"]
mod tests;
```

Keep tests in a sibling module so `learning_review.rs` stays focused on review types and helpers.

- [ ] **Step 4: Add mutation-tool deny helper tests**

In `crates/bot/src/cc/invocation.rs`, add tests inside the existing test module:

```rust
#[test]
fn disallow_background_review_mutation_tools_blocks_agent_and_writes() {
    let tools = disallow_background_review_mutation_tools(vec!["Bash".to_owned()]);
    for tool_name in [
        "Agent",
        "Write",
        "Edit",
        "MultiEdit",
        right_mcp::internal_client::PROGRESS_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL,
        right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL,
    ] {
        assert!(tools.iter().any(|tool| tool == tool_name), "missing {tool_name}");
    }
}
```

- [ ] **Step 5: Implement mutation-tool deny helper**

In `crates/bot/src/cc/invocation.rs`, add after `disallow_foreground_only_tools`:

```rust
pub(crate) fn disallow_background_review_mutation_tools(tools: Vec<String>) -> Vec<String> {
    let mut tools = disallow_foreground_only_tools(tools);
    for tool_name in ["Agent", "Write", "Edit", "MultiEdit", "NotebookEdit", "Bash"] {
        if !tools.iter().any(|tool| tool == tool_name) {
            tools.push(tool_name.to_owned());
        }
    }
    tools
}
```

- [ ] **Step 6: Run helper tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-bot learning_review
devenv shell -- cargo test -p right-bot disallow_background_review_mutation_tools
```

Expected: PASS.

- [ ] **Step 7: Commit helper slice**

Run:

```bash
devenv shell -- git add crates/bot/src/lib.rs crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs crates/bot/src/cc/invocation.rs
devenv shell -- git commit -m "feat(bot): add learned skill review helpers"
```

Expected: commit succeeds.

---

### Task 4: Learned Skill Index Collection

**Files:**
- Modify: `crates/bot/src/learning_review.rs`
- Test: `crates/bot/src/learning_review_tests.rs`

- [ ] **Step 1: Add failing host skill index test**

In `crates/bot/src/learning_review_tests.rs`, add:

```rust
#[test]
fn collect_host_rightx_skills_includes_only_learned_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".claude/skills");
    std::fs::create_dir_all(skills_dir.join("rightx-demo")).unwrap();
    std::fs::write(
        skills_dir.join("rightx-demo/SKILL.md"),
        "---\nname: rightx-demo\ndescription: Demo learned skill\n---\n# Demo\nbody\n",
    )
    .unwrap();
    std::fs::create_dir_all(skills_dir.join("custom-skill")).unwrap();
    std::fs::write(skills_dir.join("custom-skill/SKILL.md"), "# Custom\n").unwrap();

    let skills = collect_host_rightx_skill_index(dir.path()).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "rightx-demo");
    assert!(skills[0].excerpt.contains("Demo learned skill"));
}

#[test]
fn parse_sandbox_skill_index_stdout_splits_records() {
    let stdout = "\
---RIGHT-SKILL---
/sandbox/.claude/skills/rightx-one/SKILL.md
description: First skill
---RIGHT-SKILL---
/sandbox/.claude/skills/rightx-two/SKILL.md
description: Second skill
";
    let skills = parse_sandbox_skill_index_stdout(stdout);
    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0].name, "rightx-one");
    assert_eq!(skills[1].name, "rightx-two");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
devenv shell -- cargo test -p right-bot rightx_skill
```

Expected: FAIL because index helpers do not exist.

- [ ] **Step 3: Implement host and sandbox stdout parsers**

In `crates/bot/src/learning_review.rs`, add:

```rust
const SKILL_EXCERPT_MAX_BYTES: usize = 4096;
const SKILL_EXCERPT_MAX_LINES: usize = 120;

pub(crate) fn collect_host_rightx_skill_index(
    agent_dir: &std::path::Path,
) -> std::io::Result<Vec<LearnedSkillSummary>> {
    let skills_dir = agent_dir.join(".claude/skills");
    let mut skills = Vec::new();
    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(skills),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(LEARNED_SKILL_PREFIX) {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        let content = match std::fs::read_to_string(&skill_path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        skills.push(LearnedSkillSummary {
            name,
            excerpt: bounded_skill_excerpt(&content),
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn bounded_skill_excerpt(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines().take(SKILL_EXCERPT_MAX_LINES) {
        if out.len() + line.len() + 1 > SKILL_EXCERPT_MAX_BYTES {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_owned()
}

pub(crate) fn parse_sandbox_skill_index_stdout(stdout: &str) -> Vec<LearnedSkillSummary> {
    let mut skills = Vec::new();
    for record in stdout.split("---RIGHT-SKILL---").skip(1) {
        let mut lines = record.lines().filter(|line| !line.trim().is_empty());
        let Some(path) = lines.next() else {
            continue;
        };
        let Some(name) = path
            .split("/.claude/skills/")
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .filter(|name| name.starts_with(LEARNED_SKILL_PREFIX))
        else {
            continue;
        };
        let excerpt = lines.take(SKILL_EXCERPT_MAX_LINES).collect::<Vec<_>>().join("\n");
        skills.push(LearnedSkillSummary {
            name: name.to_owned(),
            excerpt: bounded_skill_excerpt(&excerpt),
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub(crate) fn sandbox_skill_index_command() -> [&'static str; 3] {
    [
        "sh",
        "-lc",
        "for f in /sandbox/.claude/skills/rightx-*/SKILL.md; do [ -f \"$f\" ] || continue; printf '%s\\n' '---RIGHT-SKILL---' \"$f\"; sed -n '1,120p' \"$f\"; done",
    ]
}
```

- [ ] **Step 4: Run parser tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-bot rightx_skill
```

Expected: PASS.

- [ ] **Step 5: Commit skill-index slice**

Run:

```bash
devenv shell -- git add crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
devenv shell -- git commit -m "feat(bot): collect learned skill review index"
```

Expected: commit succeeds.

---

### Task 5: Review Runner Seam

**Files:**
- Modify: `crates/bot/src/learning_review.rs`
- Test: `crates/bot/src/learning_review_tests.rs`
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add failing report conversion test**

In `crates/bot/src/learning_review_tests.rs`, add:

```rust
#[test]
fn output_converts_to_review_report() {
    let output = ReviewOutput::parse(serde_json::json!({
        "status": "create_candidate",
        "confidence": "high",
        "candidate_skill_name": "rightx-demo",
        "candidate_summary": "Capture this reusable workflow.",
        "evidence_refs": ["event-1"],
        "user_notice": "I found a reusable workflow candidate."
    }))
    .unwrap();

    let report = output.to_report(ReviewReportContext {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        chat_id: Some(10),
        thread_id: Some(20),
        trigger_kind: right_agent::learned_skills::ReviewTriggerKind::LearningSignal,
        telegram_notified: true,
    });

    assert_eq!(report.agent_name, "right");
    assert_eq!(report.candidate_skill_name.as_deref(), Some("rightx-demo"));
    assert!(report.telegram_notified);
}
```

- [ ] **Step 2: Implement report conversion**

In `crates/bot/src/learning_review.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReviewReportContext {
    pub(crate) agent_name: String,
    pub(crate) source_invocation_id: String,
    pub(crate) root_session_id: Option<String>,
    pub(crate) chat_id: Option<i64>,
    pub(crate) thread_id: Option<i64>,
    pub(crate) trigger_kind: right_agent::learned_skills::ReviewTriggerKind,
    pub(crate) telegram_notified: bool,
}

impl ReviewOutput {
    pub(crate) fn to_report(
        &self,
        ctx: ReviewReportContext,
    ) -> right_agent::learned_skills::SkillReviewReport {
        right_agent::learned_skills::SkillReviewReport {
            agent_name: ctx.agent_name,
            source_invocation_id: ctx.source_invocation_id,
            root_session_id: ctx.root_session_id,
            chat_id: ctx.chat_id,
            thread_id: ctx.thread_id,
            trigger_kind: ctx.trigger_kind,
            status: self.status.as_domain(),
            confidence: self.confidence.as_domain(),
            candidate_skill_name: self.candidate_skill_name.clone(),
            candidate_summary: self.candidate_summary.clone(),
            evidence_refs: self.evidence_refs.clone(),
            review_output_json: self.raw.clone(),
            telegram_notified: ctx.telegram_notified,
        }
    }
}
```

- [ ] **Step 3: Add background review runner skeleton**

In `crates/bot/src/learning_review.rs`, add a runner function that accepts a prompt and a closure. This keeps tests pure before wiring the real Claude invocation:

```rust
pub(crate) async fn run_review_with_output<F, Fut>(
    bundle: ReviewBundle,
    run_json: F,
) -> Result<ReviewOutput, String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, String>>,
{
    let prompt = build_review_prompt(&bundle);
    let raw = run_json(prompt).await?;
    ReviewOutput::parse(raw)
}
```

- [ ] **Step 4: Add runner test**

In `crates/bot/src/learning_review_tests.rs`, add:

```rust
#[tokio::test]
async fn run_review_with_output_builds_prompt_and_parses_json() {
    let bundle = ReviewBundle {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        trigger_kind: "learning_signal".to_owned(),
        accepted_signal_json: None,
        tool_iters_since_review: 2,
        turns_since_review: 1,
        skill_issue_hints_since_review: 0,
        event_timeline: vec!["event-1 user corrected OAuth flow".to_owned()],
        learning_events: vec![],
        learned_skills: vec![],
    };

    let output = run_review_with_output(bundle, |prompt| async move {
        assert!(prompt.contains("Report-only"));
        Ok(serde_json::json!({
            "status": "nothing_to_learn",
            "confidence": "low",
            "candidate_skill_name": null,
            "candidate_summary": null,
            "evidence_refs": [],
            "user_notice": null
        }))
    })
    .await
    .unwrap();

    assert_eq!(output.status, ReviewOutputStatus::NothingToLearn);
}
```

- [ ] **Step 5: Run runner tests and verify pass**

Run:

```bash
devenv shell -- cargo test -p right-bot run_review_with_output
devenv shell -- cargo test -p right-bot output_converts_to_review_report
```

Expected: PASS.

- [ ] **Step 6: Commit runner seam**

Run:

```bash
devenv shell -- git add crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
devenv shell -- git commit -m "feat(bot): add learned skill review runner seam"
```

Expected: commit succeeds.

---

### Task 6: Worker Scheduling Integration

**Files:**
- Modify: `crates/bot/src/learning_review.rs`
- Test: `crates/bot/src/learning_review_tests.rs`

- [ ] **Step 1: Add pure scheduling tests**

In `crates/bot/src/learning_review_tests.rs`, add:

```rust
#[test]
fn trigger_kind_prefers_skill_issue_signal_over_effort() {
    let trigger = select_review_trigger(true, true, false);
    assert_eq!(
        trigger,
        Some(right_agent::learned_skills::ReviewTriggerKind::SkillIssueSignal)
    );
}

#[test]
fn trigger_kind_uses_learning_signal_before_effort() {
    let trigger = select_review_trigger(true, false, true);
    assert_eq!(
        trigger,
        Some(right_agent::learned_skills::ReviewTriggerKind::LearningSignal)
    );
}

#[test]
fn trigger_kind_uses_effort_when_no_signal_exists() {
    let trigger = select_review_trigger(false, false, true);
    assert_eq!(
        trigger,
        Some(right_agent::learned_skills::ReviewTriggerKind::EffortThreshold)
    );
}

#[test]
fn review_cooldown_elapsed_handles_empty_old_and_recent_timestamps() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-05-18T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let cooldown = chrono::Duration::minutes(30);

    assert!(review_cooldown_elapsed(None, now, cooldown).unwrap());
    assert!(review_cooldown_elapsed(Some("2026-05-18T09:29:59Z"), now, cooldown).unwrap());
    assert!(!review_cooldown_elapsed(Some("2026-05-18T09:45:00Z"), now, cooldown).unwrap());

    let err = review_cooldown_elapsed(Some("not-a-date"), now, cooldown).unwrap_err();
    assert!(err.contains("last_review_at"), "{err}");
}
```

- [ ] **Step 2: Implement trigger selection and cooldown helpers**

In `crates/bot/src/learning_review.rs`, add:

```rust
pub(crate) fn select_review_trigger(
    has_learning_signal: bool,
    has_skill_issue_signal: bool,
    effort_threshold_met: bool,
) -> Option<right_agent::learned_skills::ReviewTriggerKind> {
    if has_skill_issue_signal {
        Some(right_agent::learned_skills::ReviewTriggerKind::SkillIssueSignal)
    } else if has_learning_signal {
        Some(right_agent::learned_skills::ReviewTriggerKind::LearningSignal)
    } else if effort_threshold_met {
        Some(right_agent::learned_skills::ReviewTriggerKind::EffortThreshold)
    } else {
        None
    }
}

pub(crate) fn review_cooldown_elapsed(
    last_review_at: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
    cooldown: chrono::Duration,
) -> Result<bool, String> {
    let Some(last_review_at) = last_review_at else {
        return Ok(true);
    };
    let last_review_at = chrono::DateTime::parse_from_rfc3339(last_review_at)
        .map_err(|e| format!("parse skill review last_review_at: {e:#}"))?
        .with_timezone(&chrono::Utc);
    Ok(now.signed_duration_since(last_review_at) >= cooldown)
}
```

- [ ] **Step 3: Run targeted tests**

Run:

```bash
devenv shell -- cargo test -p right-bot select_review_trigger
devenv shell -- cargo test -p right-bot review_cooldown_elapsed
devenv shell -- cargo test -p right-agent review_gate
```

Expected: PASS.

- [ ] **Step 4: Commit scheduling helper slice**

Run:

```bash
devenv shell -- git add crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
devenv shell -- git commit -m "feat(bot): add learned skill review scheduling helpers"
```

Expected: commit succeeds.

---

### Task 7: Background Review Execution And Report Storage

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`
- Modify: `crates/bot/src/learning_review.rs`
- Test: `crates/bot/src/learning_review_tests.rs`

- [ ] **Step 1: Add review schema constant**

In `crates/bot/src/learning_review.rs`, add:

```rust
pub(crate) const REVIEW_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "status": { "enum": ["nothing_to_learn", "create_candidate", "update_candidate", "failed"] },
    "confidence": { "enum": ["low", "medium", "high"] },
    "candidate_skill_name": { "type": ["string", "null"] },
    "candidate_summary": { "type": ["string", "null"] },
    "evidence_refs": { "type": "array", "items": { "type": "string" } },
    "user_notice": { "type": ["string", "null"] }
  },
  "required": ["status", "confidence", "candidate_skill_name", "candidate_summary", "evidence_refs", "user_notice"]
}"#;
```

- [ ] **Step 2: Add JSON result extraction helper**

In `crates/bot/src/learning_review.rs`, add:

```rust
pub(crate) fn parse_review_process_stdout(stdout: &str) -> Result<ReviewOutput, String> {
    let value: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("parse review stdout JSON: {e:#}"))?;
    let result = value.get("result").cloned().unwrap_or(value);
    ReviewOutput::parse(result)
}
```

Add test:

```rust
#[test]
fn parse_review_process_stdout_reads_result_object() {
    let stdout = r#"{"result":{"status":"nothing_to_learn","confidence":"low","candidate_skill_name":null,"candidate_summary":null,"evidence_refs":[],"user_notice":null}}"#;
    let output = parse_review_process_stdout(stdout).unwrap();
    assert_eq!(output.status, ReviewOutputStatus::NothingToLearn);
}
```

- [ ] **Step 3: Add worker scheduling helper with real review execution**

In `crates/bot/src/telegram/worker.rs`, after the block that records nudge signals and before `Ok(CcReply { ... })`, add this call:

```rust
            let accepted_review_signal_json = match (
                reply_output.learning_signal.as_ref(),
                reply_output.skill_issue_signal.as_ref(),
            ) {
                (Some(signal), None) | (None, Some(signal)) => Some(signal.to_string()),
                _ => None,
            };
            maybe_spawn_learned_skill_review(
                &conn,
                &ctx,
                chat_id,
                eff_thread_id,
                &session_uuid,
                learning_invocation_id.as_deref(),
                reply_output.learning_signal.is_some(),
                reply_output.skill_issue_signal.is_some(),
                accepted_review_signal_json,
            )
            .await;
```

Add these helpers below `remove_sandbox_progress_config_file`:

```rust
fn review_today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn load_skill_review_gate_snapshot(
    conn: &rusqlite::Connection,
    agent_name: &str,
) -> Result<(Option<String>, i64, i64, i64), rusqlite::Error> {
    right_agent::learned_skills::ensure_nudge_state(conn, agent_name)?;
    conn.query_row(
        "SELECT last_review_at, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review \
         FROM skill_nudge_state WHERE agent_name = ?1",
        [agent_name],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
}

async fn maybe_spawn_learned_skill_review(
    conn: &rusqlite::Connection,
    ctx: &WorkerContext,
    chat_id: i64,
    eff_thread_id: i64,
    root_session_id: &str,
    source_invocation_id: Option<&str>,
    has_learning_signal: bool,
    has_skill_issue_signal: bool,
    accepted_signal_json: Option<String>,
) {
    let Some(source_invocation_id) = source_invocation_id else {
        return;
    };
    let (
        last_review_at,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
    ) = match load_skill_review_gate_snapshot(conn, &ctx.agent_name) {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learned-skill review gate snapshot load failed: {e:#}");
            return;
        }
    };
    let cooldown_elapsed = match crate::learning_review::review_cooldown_elapsed(
        last_review_at.as_deref(),
        chrono::Utc::now(),
        chrono::Duration::minutes(30),
    ) {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learned-skill review cooldown check failed: {e}");
            return;
        }
    };
    let today = review_today_utc();
    let gate = match right_agent::learned_skills::review_gate_decision(
        conn,
        &ctx.agent_name,
        right_agent::learned_skills::ReviewGateInput {
            has_signal: has_learning_signal || has_skill_issue_signal,
            today: &today,
            cooldown_elapsed,
            daily_limit: 12,
        },
    ) {
        Ok(gate) => gate,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learned-skill review gate failed: {e:#}");
            return;
        }
    };
    let right_agent::learned_skills::ReviewGateDecision::Start(gated_trigger) = gate else {
        return;
    };
    let effort_threshold_met =
        matches!(gated_trigger, right_agent::learned_skills::ReviewTriggerKind::EffortThreshold);
    let Some(trigger_kind) = crate::learning_review::select_review_trigger(
        has_learning_signal,
        has_skill_issue_signal,
        effort_threshold_met,
    ) else {
        return;
    };
    if let Err(e) = right_agent::learned_skills::mark_review_started(conn, &ctx.agent_name, &today) {
        tracing::warn!(agent = %ctx.agent_name, "learned-skill review start mark failed: {e:#}");
        return;
    }

    let agent_name = ctx.agent_name.clone();
    let agent_dir = ctx.agent_dir.clone();
    let bot = ctx.bot.clone();
    let model = crate::snapshot_model(&ctx.model);
    let ssh_config_path = ctx.ssh_config_path.clone();
    let resolved_sandbox = ctx.resolved_sandbox.clone();
    let debug = std::sync::Arc::clone(&ctx.debug);
    let source_invocation_id = source_invocation_id.to_owned();
    let root_session_id = root_session_id.to_owned();
    let tg_chat_id = teloxide::types::ChatId(chat_id);

    std::mem::drop(tokio::spawn(async move {
        let report_result = run_background_learned_skill_review(
            &agent_name,
            &agent_dir,
            source_invocation_id.clone(),
            root_session_id,
            chat_id,
            eff_thread_id,
            trigger_kind,
            model,
            ssh_config_path,
            resolved_sandbox,
            debug,
            accepted_signal_json,
            tool_iters_since_review,
            turns_since_review,
            skill_issue_hints_since_review,
        )
        .await;
        match report_result {
            Ok((report, notice)) => {
                let conn = match right_db::open_connection(&agent_dir, false) {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(agent = %agent_name, "learned-skill review db reopen failed: {e:#}");
                        return;
                    }
                };
                if let Err(e) = right_agent::learned_skills::insert_skill_review_report(&conn, &report) {
                    tracing::warn!(agent = %agent_name, "learned-skill review report insert failed: {e:#}");
                }
                if let Err(e) = right_agent::learned_skills::mark_review_finished(
                    &conn,
                    &agent_name,
                    report.status,
                    report.status != right_agent::learned_skills::ReviewStatus::Failed,
                ) {
                    tracing::warn!(agent = %agent_name, "learned-skill review finish mark failed: {e:#}");
                }
                if let Some(notice) = notice {
                    let _ = send_tg(&bot, tg_chat_id, eff_thread_id, &notice).await;
                }
            }
            Err(e) => {
                tracing::warn!(agent = %agent_name, "learned-skill background review failed: {e:#}");
                if let Ok(conn) = right_db::open_connection(&agent_dir, false) {
                    let _ = right_agent::learned_skills::mark_review_finished(
                        &conn,
                        &agent_name,
                        right_agent::learned_skills::ReviewStatus::Failed,
                        false,
                    );
                }
            }
        }
    }));
}
```

- [ ] **Step 4: Add background review runner function**

In `crates/bot/src/telegram/worker.rs`, add below `maybe_spawn_learned_skill_review`:

```rust
async fn run_background_learned_skill_review(
    agent_name: &str,
    agent_dir: &Path,
    source_invocation_id: String,
    root_session_id: String,
    chat_id: i64,
    thread_id: i64,
    trigger_kind: right_agent::learned_skills::ReviewTriggerKind,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    resolved_sandbox: Option<String>,
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
    accepted_signal_json: Option<String>,
    tool_iters_since_review: i64,
    turns_since_review: i64,
    skill_issue_hints_since_review: i64,
) -> anyhow::Result<(right_agent::learned_skills::SkillReviewReport, Option<String>)> {
    let learned_skills = if ssh_config_path.is_some() {
        collect_sandbox_review_skill_index(resolved_sandbox.as_deref()).await?
    } else {
        crate::learning_review::collect_host_rightx_skill_index(agent_dir)
            .map_err(|e| anyhow::anyhow!("collect host learned skills: {e:#}"))?
    };
    let mut event_timeline =
        crate::learning_review::collect_stream_event_timeline(agent_dir, &root_session_id, 80)
            .map_err(|e| anyhow::anyhow!("collect review event timeline: {e:#}"))?;
    if event_timeline.is_empty() {
        event_timeline.push(format!(
            "event-1 foreground invocation {} completed; stream log unavailable or empty",
            source_invocation_id
        ));
    }
    let learning_events = load_review_learning_events(agent_dir, &source_invocation_id)
        .map_err(|e| anyhow::anyhow!("load review learning events: {e:#}"))?;
    let bundle = crate::learning_review::ReviewBundle {
        agent_name: agent_name.to_owned(),
        source_invocation_id: source_invocation_id.clone(),
        root_session_id: Some(root_session_id.clone()),
        trigger_kind: trigger_kind.as_str().to_owned(),
        accepted_signal_json,
        tool_iters_since_review,
        turns_since_review,
        skill_issue_hints_since_review,
        event_timeline,
        learning_events,
        learned_skills,
    };
    let prompt = crate::learning_review::build_review_prompt(&bundle);
    let disallowed_tools = crate::cc::invocation::disallow_background_review_mutation_tools(
        crate::cc::invocation::baseline_disallowed_tools(),
    );
    let invocation = crate::cc::invocation::ClaudeInvocation {
        mcp_config_path: None,
        json_schema: Some(crate::learning_review::REVIEW_SCHEMA_JSON.to_owned()),
        output_format: crate::cc::invocation::OutputFormat::Json,
        model,
        max_budget_usd: Some(0.50),
        max_turns: Some(8),
        resume_session_id: None,
        new_session_id: None,
        fork_session: false,
        allowed_tools: vec!["Read".to_owned(), "Glob".to_owned(), "Grep".to_owned(), "LS".to_owned()],
        disallowed_tools,
        extra_args: vec![],
        prompt: Some(prompt),
        debug_flag: Some(debug),
    };
    let args = invocation.into_args();
    let mut cmd = crate::cc::invocation::build_claude_command(
        &args,
        agent_dir,
        ssh_config_path.as_deref(),
        resolved_sandbox.as_deref(),
    );
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = right_process::ProcessGroupChild::spawn(cmd)
        .map_err(|e| anyhow::anyhow!("spawn background review claude: {e:#}"))?;
    let output = tokio::time::timeout(
        tokio::time::Duration::from_secs(180),
        child.wait_with_output(),
    )
        .await
        .map_err(|_| anyhow::anyhow!("background review claude timed out"))?
        .map_err(|e| anyhow::anyhow!("wait for background review claude: {e:#}"))?;
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "background review claude exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| anyhow::anyhow!("background review stdout utf8: {e:#}"))?;
    let review_output = crate::learning_review::parse_review_process_stdout(&stdout)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let notice = review_output.should_notify_user().then(|| {
        review_output
            .user_notice
            .clone()
            .unwrap_or_else(|| "I found a reusable workflow candidate and recorded it for review.".to_owned())
    });
    let report = review_output.to_report(crate::learning_review::ReviewReportContext {
        agent_name: agent_name.to_owned(),
        source_invocation_id,
        root_session_id: Some(root_session_id),
        chat_id: Some(chat_id),
        thread_id: Some(thread_id),
        trigger_kind,
        telegram_notified: notice.is_some(),
    });
    Ok((report, notice))
}

fn load_review_learning_events(
    agent_dir: &Path,
    source_invocation_id: &str,
) -> anyhow::Result<Vec<String>> {
    let conn = right_db::open_connection(agent_dir, false)?;
    let mut stmt = conn.prepare(
        "SELECT action, skill_name, phase, COALESCE(status, ''), COALESCE(summary, '') \
         FROM skill_learning_events WHERE invocation_id = ?1 ORDER BY id LIMIT 20",
    )?;
    let rows = stmt.query_map([source_invocation_id], |row| {
        let action: String = row.get(0)?;
        let skill_name: String = row.get(1)?;
        let phase: String = row.get(2)?;
        let status: String = row.get(3)?;
        let summary: String = row.get(4)?;
        Ok(format!(
            "{} {} {} status={} summary={}",
            phase, action, skill_name, status, summary
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}
```

- [ ] **Step 5: Add sandbox skill index collector**

In `crates/bot/src/telegram/worker.rs`, add:

```rust
async fn collect_sandbox_review_skill_index(
    sandbox_name: Option<&str>,
) -> anyhow::Result<Vec<crate::learning_review::LearnedSkillSummary>> {
    let sandbox_name = sandbox_name.ok_or_else(|| anyhow::anyhow!("sandbox name unresolved"))?;
    let mtls_dir = match right_openshell::openshell::preflight_check() {
        right_openshell::openshell::OpenShellStatus::Ready(dir) => dir,
        status => return Err(anyhow::anyhow!("OpenShell not ready for review skill index: {status:?}")),
    };
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let sandbox_id = right_openshell::openshell::resolve_sandbox_id(&mut client, sandbox_name)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let cmd = crate::learning_review::sandbox_skill_index_command();
    let (stdout, exit_code) = right_openshell::openshell::exec_in_sandbox(
        &mut client,
        &sandbox_id,
        &cmd,
        right_openshell::openshell::DEFAULT_EXEC_TIMEOUT_SECS,
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    if exit_code != 0 {
        return Err(anyhow::anyhow!("sandbox skill index command exited {exit_code}: {stdout}"));
    }
    Ok(crate::learning_review::parse_sandbox_skill_index_stdout(&stdout))
}
```

- [ ] **Step 6: Run targeted tests and compile check**

Run:

```bash
devenv shell -- cargo test -p right-bot parse_review_process_stdout
devenv shell -- cargo test -p right-bot learning_review
devenv shell -- cargo check -p right-bot
```

Expected: PASS.

- [ ] **Step 7: Commit execution slice**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/worker.rs crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
devenv shell -- git commit -m "feat(bot): run learned skill background review"
```

Expected: commit succeeds.

---

### Task 8: Docs And Prompt Sync

**Files:**
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update sessions architecture**

In `docs/architecture/sessions.md`, extend the learned-skill foreground paragraph with:

```markdown
Background learned-skill review is separate from foreground progress. After a
foreground turn completes, the bot may start a `BackgroundReview` Claude Code
invocation when a learned-skill signal exists or the per-agent effort counter
reaches the review interval. The background review receives a bounded report
bundle, denies mutation tools, stores a structured report, and sends Telegram
only for high-confidence candidates.
```

- [ ] **Step 2: Update MCP architecture**

In `docs/architecture/mcp.md`, add under learned skill tools:

```markdown
Stage 2 background learned-skill review is report-only. Background review
invocations do not expose `mcp__right__skill_learning_start` or
`mcp__right__skill_learning_finish`; those tools remain foreground learning
protocol tools in Stage 2.
```

- [ ] **Step 3: Update PROMPT_SYSTEM.md**

In `PROMPT_SYSTEM.md`, add to the learned-skill section:

```markdown
Background learned-skill review is report-only in Stage 2. It may record
high-confidence create/update candidates from a completed foreground turn, but
it must not create, patch, archive, or delete skill package files.
```

- [ ] **Step 4: Run docs/prompt checks**

Run:

```bash
devenv shell -- rg -n "mcp__right__skill_learning_start|mcp__right__skill_learning_finish|BackgroundReview|report-only" docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md
devenv shell -- cargo test -p right-codegen agent_def
```

Expected: search output includes the new report-only language; tests pass.

- [ ] **Step 5: Commit docs slice**

Run:

```bash
devenv shell -- git add docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md
devenv shell -- git commit -m "docs: document learned skill background review"
```

Expected: commit succeeds.

---

### Task 9: Final Verification

**Files:**
- Read: `git status --short`
- Read: recent commits

- [ ] **Step 1: Run learned-skill targeted suite**

Run:

```bash
devenv shell -- cargo test -p right-db skill_review
devenv shell -- cargo test -p right-agent review_
devenv shell -- cargo test -p right-bot learning_review
devenv shell -- cargo test -p right-bot disallow_background_review_mutation_tools
```

Expected: all pass.

- [ ] **Step 2: Run final workspace verification**

Run:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

Expected: both commands exit with status 0.

- [ ] **Step 3: Verify no stale tool references**

Run:

```bash
devenv shell -- rg -n "skill_learning_start|skill_learning_finish|BackgroundReview|rightx-" crates/bot crates/right-agent crates/right-db docs/architecture PROMPT_SYSTEM.md
```

Expected: references are intentional and use `mcp__right__` prefixes in agent-facing docs.

- [ ] **Step 4: Inspect git status**

Run:

```bash
devenv shell -- git status --short
```

Expected: clean except for unrelated pre-existing user changes. Do not revert unrelated files.
