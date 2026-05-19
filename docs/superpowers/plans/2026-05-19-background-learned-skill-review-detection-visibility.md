# Background Learned-Skill Review Detection Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make background learned-skill review detect reusable candidates more reliably and make review outcomes diagnosable without changing Stage 2 into automatic skill mutation.

**Architecture:** Keep background review report-only. Fix the reviewer prompt so it matches the existing Stage 2 design rules, add tests proving Telegram notice persistence/sending behavior, and add explicit completion logging for every stored review report. Do not send Telegram for `nothing_to_learn` by default; Telegram remains reserved for high-confidence `create_candidate` and `update_candidate` reports with `user_notice`.

**Tech Stack:** Rust 2024, `right-bot`, `right-agent::learned_skills`, `rusqlite`, Claude Code JSON structured output, existing Telegram worker tests.

---

## Diagnosis To Preserve

Observed local runtime state for agent `agent-b` on 2026-05-19:

```sql
SELECT id, source_invocation_id, root_session_id, trigger_kind, status, confidence,
       candidate_skill_name, telegram_notified, created_at, substr(review_output_json,1,700)
FROM skill_review_reports
ORDER BY id DESC
LIMIT 3;
```

Result summary:

```text
3 | effort_threshold | nothing_to_learn | medium | candidate NULL | telegram_notified 0
2 | effort_threshold | nothing_to_learn | medium | candidate NULL | telegram_notified 0
1 | effort_threshold | nothing_to_learn | high   | candidate NULL | telegram_notified 0
```

The latest report did include evidence:

```json
{
  "status": "nothing_to_learn",
  "confidence": "medium",
  "candidate_skill_name": null,
  "candidate_summary": null,
  "evidence_refs": [
    "event-12 (cron_create right-agent-release-tracker)",
    "event-20/21 (gh api releases/tags)",
    "event-23 (memory_retain last seen tag)"
  ],
  "user_notice": null
}
```

`skill_nudge_signals` was empty, so none of these reviews were triggered by an accepted foreground `learning_signal` or `skill_issue_signal`; they ran by `effort_threshold`.

Important conclusion:

- The background reviewer did run.
- Telegram did not fail.
- The reviewer saw events but classified them as `nothing_to_learn`.
- Current design sends Telegram only when `ReviewOutput::should_notify_user()` is true:
  `status in {create_candidate, update_candidate}` AND `confidence = high` AND `user_notice IS NOT NULL`.

The fix is not "send Telegram for every review". The fix is to improve candidate detection and make no-notice cases observable in logs/reports.

## File Structure

- Modify: `crates/bot/src/learning_review.rs`
  - Strengthen `build_review_prompt()` with the decision rules already required by `docs/superpowers/specs/2026-05-18-background-learned-skill-review-design.md`.
- Modify: `crates/bot/src/learning_review_tests.rs`
  - Add regression coverage for the missing prompt rules.
- Modify: `crates/bot/src/telegram/worker.rs`
  - Add a success-path test for notice sending and `telegram_notified = true`.
  - Add structured `tracing::info!` on every successfully stored review report.
- Modify: `docs/architecture/sessions.md`
  - Keep the description aligned with the exact notification rule and mention that non-notified reports are visible in logs and `skill_review_reports`.
- Modify: `docs/architecture/mcp.md`
  - Keep Stage 2 report-only language aligned with the prompt.
- Modify: `PROMPT_SYSTEM.md`
  - Keep the generated prompting reference aligned with the background review contract.

## Task 1: Strengthen Reviewer Prompt Rules

**Files:**
- Modify: `crates/bot/src/learning_review_tests.rs`
- Modify: `crates/bot/src/learning_review.rs`

- [ ] **Step 1: Add the failing prompt regression test**

In `crates/bot/src/learning_review_tests.rs`, extend `review_prompt_says_report_only_and_nothing_to_learn_is_normal()` with these assertions after the existing `nothing_to_learn is normal` assertion:

```rust
assert!(prompt.contains("Candidates must be reusable across future sessions"));
assert!(prompt.contains("Do not preserve one-off task narrative"));
assert!(prompt.contains("Do not make persistent negative claims from transient tool failures"));
assert!(prompt.contains("Prefer update candidates for existing rightx-* skills"));
```

- [ ] **Step 2: Run the targeted failing test**

Run:

```bash
devenv shell -- cargo test -p right-bot review_prompt_says_report_only_and_nothing_to_learn_is_normal
```

Expected: FAIL. The failure should show at least one missing prompt substring.

- [ ] **Step 3: Implement the prompt rule block**

In `crates/bot/src/learning_review.rs`, update `build_review_prompt()` immediately after the existing report-only paragraph:

```rust
prompt.push_str(
    "Decision rules:\n\
     - Candidates must be reusable across future sessions, not a summary of this one task.\n\
     - Do not preserve one-off task narrative in candidate summaries.\n\
     - Do not make persistent negative claims from transient tool failures.\n\
     - Prefer update candidates for existing rightx-* skills when the evidence refines an installed learned skill.\n\
     - Use create_candidate when repeated tool patterns or setup workflows are reusable and no existing rightx-* skill fits.\n\
     - Use nothing_to_learn when the evidence is only normal task progress, isolated facts, or one-time content.\n\n",
);
```

Do not remove the existing report-only paragraph. It protects the Stage 2 mutation boundary.

- [ ] **Step 4: Re-run the targeted test**

Run:

```bash
devenv shell -- cargo test -p right-bot review_prompt_says_report_only_and_nothing_to_learn_is_normal
```

Expected: PASS.

- [ ] **Step 5: Commit Task 1**

Run:

```bash
devenv shell -- git add crates/bot/src/learning_review.rs crates/bot/src/learning_review_tests.rs
devenv shell -- git commit -m "fix(bot): strengthen learned-skill review prompt"
```

## Task 2: Prove Telegram Notice Success Path

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add a failing success-path test**

In `crates/bot/src/telegram/worker.rs`, inside the existing test module near `record_successful_background_review_persists_notified_false_when_send_fails()`, add:

```rust
#[tokio::test]
async fn record_successful_background_review_sends_notice_and_persists_notified_true() {
    let temp = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(temp.path(), true).unwrap();
    right_agent::learned_skills::ensure_nudge_state(&conn, "right").unwrap();
    conn.execute(
        "UPDATE skill_nudge_state SET review_running = 1 WHERE agent_name = 'right'",
        [],
    )
    .unwrap();
    drop(conn);

    let report = SkillReviewReport {
        agent_name: "right".to_owned(),
        source_invocation_id: "inv-1".to_owned(),
        root_session_id: Some("session-1".to_owned()),
        chat_id: Some(10),
        thread_id: Some(20),
        trigger_kind: right_agent::learned_skills::ReviewTriggerKind::LearningSignal,
        status: ReviewStatus::CreateCandidate,
        confidence: right_agent::learned_skills::ReviewConfidence::High,
        candidate_skill_name: Some("rightx-demo".to_owned()),
        candidate_summary: Some("Demo candidate".to_owned()),
        evidence_refs: vec!["event-1".to_owned()],
        review_output_json: serde_json::json!({
            "status": "create_candidate",
            "confidence": "high",
            "candidate_skill_name": "rightx-demo",
            "candidate_summary": "Demo candidate",
            "evidence_refs": ["event-1"],
            "user_notice": "notice"
        }),
        telegram_notified: false,
    };

    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sent_for_closure = std::sync::Arc::clone(&sent);

    record_successful_background_review(
        temp.path(),
        "right",
        report,
        Some("notice".to_owned()),
        move |notice| {
            let sent_for_future = std::sync::Arc::clone(&sent_for_closure);
            async move {
                sent_for_future.lock().unwrap().push(notice);
                Ok::<(), &'static str>(())
            }
        },
    )
    .await;

    assert_eq!(sent.lock().unwrap().as_slice(), ["notice"]);

    let conn = right_db::open_connection(temp.path(), false).unwrap();
    let row: (i64, i64, String) = conn
        .query_row(
            "SELECT r.telegram_notified, s.review_running, s.last_review_status \
             FROM skill_review_reports r \
             JOIN skill_nudge_state s ON s.agent_name = r.agent_name \
             WHERE r.agent_name = 'right'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(row, (1, 0, "create_candidate".to_owned()));
}
```

- [ ] **Step 2: Run the targeted test**

Run:

```bash
devenv shell -- cargo test -p right-bot record_successful_background_review_sends_notice_and_persists_notified_true
```

Expected: PASS if the existing send path is correct. If it fails, fix only `record_successful_background_review()`; do not change Telegram notification policy.

- [ ] **Step 3: Commit Task 2**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/worker.rs
devenv shell -- git commit -m "test(bot): cover learned-skill review notice success"
```

## Task 3: Add Review Completion Visibility

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs`

- [ ] **Step 1: Add structured completion logging**

In `crates/bot/src/telegram/worker.rs`, inside `record_successful_background_review()` after the `mark_review_finished(...)` block, add:

```rust
tracing::info!(
    agent = %agent_name,
    source_invocation_id = %report.source_invocation_id,
    trigger_kind = %report.trigger_kind.as_str(),
    status = %report.status.as_str(),
    confidence = %report.confidence.as_str(),
    candidate_skill_name = report.candidate_skill_name.as_deref().unwrap_or(""),
    telegram_notified = report.telegram_notified,
    "learned-skill background review completed"
);
```

Keep failure logging as `warn!`.

- [ ] **Step 2: Run a targeted compile/test check**

Run:

```bash
devenv shell -- cargo test -p right-bot record_successful_background_review
```

Expected: PASS. This filter should run the existing failure-path test and the new success-path test.

- [ ] **Step 3: Commit Task 3**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/worker.rs
devenv shell -- git commit -m "fix(bot): log learned-skill review outcomes"
```

## Task 4: Align Architecture And Prompt Docs

**Files:**
- Modify: `docs/architecture/sessions.md`
- Modify: `docs/architecture/mcp.md`
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Update `docs/architecture/sessions.md`**

In the background learned-skill review paragraph, keep the existing flow and append this sentence:

```markdown
Reports that do not notify Telegram are still persisted in `skill_review_reports`
and logged with trigger, status, confidence, candidate name, and
`telegram_notified`; `nothing_to_learn` remains silent for users by default.
```

- [ ] **Step 2: Update `docs/architecture/mcp.md`**

In the Stage 2 background learned-skill review paragraph, append:

```markdown
The reviewer prompt includes candidate decision rules: candidates must be
reusable across future sessions, one-off task narrative must not become a skill,
transient tool failures must not become persistent negative claims, and existing
`rightx-*` skills should be updated before creating new candidates.
```

- [ ] **Step 3: Update `PROMPT_SYSTEM.md`**

In the Background learned-skill review section, append:

```markdown
The reviewer prompt explicitly prefers reusable future-session workflows,
rejects one-off task narrative, avoids persistent claims from transient failures,
and prefers update candidates for existing `rightx-*` skills when applicable.
```

- [ ] **Step 4: Verify documentation mentions the new rules**

Run:

```bash
devenv shell -- rg -n "reusable across future sessions|one-off task narrative|transient tool failures|telegram_notified" docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md
```

Expected: output includes all three files.

- [ ] **Step 5: Commit Task 4**

Run:

```bash
devenv shell -- git add docs/architecture/sessions.md docs/architecture/mcp.md PROMPT_SYSTEM.md
devenv shell -- git commit -m "docs: clarify learned-skill review visibility"
```

## Task 5: Runtime Verification Against `agent-b`

**Files:**
- No repository file changes in this task.

- [ ] **Step 1: Confirm current `agent-b` baseline**

Run:

```bash
devenv shell -- sqlite3 -readonly -header -column /Users/developer/.right/agents/agent-b/data.db "SELECT agent_name, tool_iters_since_review, turns_since_review, skill_issue_hints_since_review, review_running, creation_review_interval, daily_review_count, daily_review_date, last_review_at, last_review_status FROM skill_nudge_state"
```

Expected: one row for `agent-b`. `review_running` must be `0` before manual smoke testing.

- [ ] **Step 2: Restart `agent-b` through the normal platform path**

Run:

```bash
devenv shell -- cargo run -p right -- restart agent-b
```

Expected: command exits 0 and `agent-b-bot` restarts.

- [ ] **Step 3: Trigger a foreground task with a reusable workflow**

In Telegram, ask `agent-b` to perform a task with an obviously reusable workflow. Use this exact shape so the smoke is comparable:

```text
Create or update a recurring GitHub release tracker for a repository, fetch the latest release/tag, store the last seen version in memory, and explain the repeatable workflow you used.
```

Expected: the foreground task completes. If the foreground reply emits a `learning_signal`, the next background review should use `learning_signal`; otherwise it should run by `effort_threshold` once the counter passes 15.

- [ ] **Step 4: Inspect the newest review report**

Run:

```bash
devenv shell -- sqlite3 -readonly -header -column /Users/developer/.right/agents/agent-b/data.db "SELECT id, source_invocation_id, trigger_kind, status, confidence, candidate_skill_name, telegram_notified, created_at, substr(review_output_json,1,700) AS output FROM skill_review_reports ORDER BY id DESC LIMIT 5"
```

Expected for a good smoke: a new row appears. For a reusable workflow, preferred result is `create_candidate` or `update_candidate` with `confidence = high`, a `rightx-*` candidate name, non-empty `user_notice`, and `telegram_notified = 1`. If the model still returns `nothing_to_learn`, preserve the JSON output in the implementation notes and tighten only the prompt decision rules; do not change the Telegram policy.

- [ ] **Step 5: Check process logs for completion visibility**

Run:

```bash
devenv shell -- rg -n "learned-skill background review completed|learned-skill background review failed|learned-skill review Telegram notice failed" /Users/developer/.right/logs/agent-b.log.2026-05-19
```

Expected: at least one completion log line for the new review, including status, confidence, trigger, and `telegram_notified`.

## Final Verification

- [ ] **Step 1: Run targeted learned-skill suites**

Run:

```bash
devenv shell -- cargo test -p right-bot learning_review
devenv shell -- cargo test -p right-bot record_successful_background_review
devenv shell -- cargo test -p right-agent learned_skills
```

Expected: all targeted tests pass.

- [ ] **Step 2: Run full workspace tests**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: PASS. If there are pre-existing failures, record the exact failing tests and prove they are unrelated to these changes.

- [ ] **Step 3: Check final diff and status**

Run:

```bash
devenv shell -- git diff --check
devenv shell -- git status --short
```

Expected: `git diff --check` exits 0. `git status --short` shows no uncommitted changes after the task commits.

## Non-Goals

- Do not make background review mutate skill files.
- Do not call `mcp__right__skill_learning_start` or `mcp__right__skill_learning_finish` from background review.
- Do not send Telegram messages for every `nothing_to_learn` report.
- Do not bypass cooldown, daily limit, or `review_running` gates.
- Do not add Hermes Curator behavior in this fix.
