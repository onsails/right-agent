# Cron Skill Learning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let recurring cron runs feed the existing skill-learning pipeline (Haiku prefilter → probe-writer fork) so a stable repeating task can be codified into a `rightx-*` skill instead of regressing run-to-run.

**Architecture:** Extract the worker's inline post-turn learning block into a shared `learning_pipeline::run_post_turn(ctx, anchor)` function. Worker keeps its `PromptMode::Normal` precondition at the call site; cron applies a `Recurring`-only precondition and builds a `ProbeAnchor` whose `main_session_uuid` is the cron run's session id. Everything downstream (budget gate, prefilter, probe-writer fork) is reused verbatim.

**Tech Stack:** Rust (edition 2024), tokio, `right-db` (turso), existing `learning_prefilter` / `learning_probe_writer` modules.

**Spec:** `docs/superpowers/specs/2026-06-02-cron-skill-learning-design.md`

---

## Pre-flight

Per project memory (`project_rightclaw_checkout_churn`): do this work in a git worktree under `.worktrees/`, land via fast-forward push to `origin/master`. Do **not** create a branch on the shared checkout.

Baseline before starting:

```
devenv shell -- cargo test -p bot --no-run
```

Expected: compiles. Record any pre-existing failures (see project memory `project_flaky_tests_parallel_load` — the cc/invocation pid race and dashboard warn-count tests flake under load; re-run isolated before blaming your change).

---

## File Structure

- **Create:** `crates/bot/src/learning_pipeline.rs` — `PostTurnLearningCtx` struct + `run_post_turn` async fn + moved helpers `record_budget_skip`, `summary_first_line`. One responsibility: run the budget-gate→prefilter→probe-writer sequence for one captured turn.
- **Modify:** `crates/bot/src/lib.rs` — register the module; reorder `session_locks` creation above the cron spawn; pass `config.learning.clone()` + `session_locks` into `run_cron_task`.
- **Modify:** `crates/bot/src/telegram/worker.rs` — replace the inline block (`2128-2272`) with a `run_post_turn` call; delete the moved helpers; fix the worker test that referenced `record_budget_skip`.
- **Modify:** `crates/bot/src/cron.rs` — thread `LearningConfig` + `session_locks` through `run_cron_task` → `run_job_loop` → `execute_job`; add `parse_result_stats`; build the cron `ProbeAnchor` on recurring-success and call `run_post_turn`.
- **Modify:** `docs/architecture/learning.md`, `ARCHITECTURE.md` — document the second trigger source.

---

## Task 1: Extract the shared learning pipeline module (pure move, no behavior change)

**Files:**
- Create: `crates/bot/src/learning_pipeline.rs`
- Modify: `crates/bot/src/lib.rs` (add `pub(crate) mod learning_pipeline;`)

- [ ] **Step 1: Create the module with the context struct and function.**

Create `crates/bot/src/learning_pipeline.rs`. The body is the worker block from `worker.rs:2155-2271` (the `async move` body), parameterized by an owned context struct. It does NOT call `tokio::spawn` — callers own spawning.

```rust
//! Shared post-turn skill-learning pipeline: budget gate → Haiku prefilter →
//! probe-writer fork. Called by the foreground worker (Normal turns) and by
//! recurring cron runs. Pure sequence; callers wrap it in `tokio::spawn`.

use std::path::PathBuf;
use std::sync::Arc;

use crate::telegram::worker::ProbeAnchor;

/// Everything `run_post_turn` needs, owned so it can move into a spawned task.
pub(crate) struct PostTurnLearningCtx {
    pub agent_dir: PathBuf,
    pub agent_db_dir: PathBuf,
    pub agent_name: String,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub internal_client: Arc<right_mcp::internal_client::InternalClient>,
    pub session_locks: crate::telegram::SessionLocks,
    pub debug_flag: Arc<std::sync::atomic::AtomicBool>,
    /// Resolved Haiku model for the prefilter.
    pub prefilter_model: String,
    pub probe_writer_enabled: bool,
    /// Explicit probe-writer model override (from learning config).
    pub probe_writer_model_override: Option<String>,
    /// Fallback probe-writer model (the agent's current model) used when the
    /// override is absent.
    pub probe_writer_model_fallback: Option<String>,
    pub daily_budget: f64,
    pub baseline_window_days: u32,
    pub baseline_min_sample: u32,
}

/// Run the budget gate, prefilter, and (on a non-Skip decision) the
/// probe-writer fork for one captured turn. All failure paths log and return;
/// never propagates (the caller is fire-and-forget and must not be disrupted).
pub(crate) async fn run_post_turn(ctx: PostTurnLearningCtx, anchor: ProbeAnchor) {
    let now_utc = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let conn = match right_db::open_connection(&ctx.agent_db_dir, false).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learning pipeline: open_connection failed: {e:#}");
            return;
        }
    };
    let today_spend = match crate::learning_prefilter::today_spend_usd(&conn, &now_utc).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "learning pipeline: today_spend query failed: {e:#}");
            return;
        }
    };
    if today_spend >= ctx.daily_budget {
        tracing::debug!(
            agent = %ctx.agent_name,
            spend = today_spend,
            budget = ctx.daily_budget,
            "learning pipeline skipped: daily budget exhausted"
        );
        record_budget_skip(&conn, &ctx.agent_name, anchor.chat_id, anchor.thread_id).await;
        return;
    }

    let prefilter_ctx = crate::learning_prefilter::PrefilterContext {
        agent_dir: ctx.agent_dir.clone(),
        agent_db_dir: ctx.agent_db_dir.clone(),
        agent_name: ctx.agent_name.clone(),
        ssh_config_path: ctx.ssh_config_path.clone(),
        resolved_sandbox: ctx.resolved_sandbox.clone(),
        model: ctx.prefilter_model.clone(),
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        baseline_window_days: ctx.baseline_window_days,
        baseline_min_sample: ctx.baseline_min_sample,
    };
    let decision = crate::learning_prefilter::run(prefilter_ctx, anchor.clone()).await;
    let hint = match decision {
        crate::learning_prefilter::PrefilterDecision::Skip { reason } => {
            tracing::debug!(reason = %reason, "prefilter skipped");
            return;
        }
        crate::learning_prefilter::PrefilterDecision::PatchExisting { target_skill, reason } => {
            crate::learning_probe_writer::ProbeWriterHint::PatchExisting { target_skill, reason }
        }
        crate::learning_prefilter::PrefilterDecision::CreateNew { topic_hint, reason } => {
            crate::learning_probe_writer::ProbeWriterHint::CreateNew { topic_hint, reason }
        }
    };
    if !ctx.probe_writer_enabled {
        return;
    }
    let probe_writer_model = match ctx
        .probe_writer_model_override
        .or(ctx.probe_writer_model_fallback)
    {
        Some(m) if !m.is_empty() => m,
        _ => {
            tracing::warn!(agent = %ctx.agent_name, "probe-writer model unresolved, skipping");
            return;
        }
    };

    let skill_index = match crate::learning_prefilter::collect_rightx_skill_index(
        ctx.resolved_sandbox.as_deref(),
        &ctx.agent_dir,
    )
    .await
    {
        Ok(entries) => entries
            .into_iter()
            .map(|s| format!("- {}: {}", s.name, summary_first_line(&s.excerpt)))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => {
            tracing::warn!(agent = %ctx.agent_name, "collect_rightx_skill_index failed: {e:#}");
            String::new()
        }
    };

    let writer_ctx = crate::learning_probe_writer::ProbeWriterContext {
        agent_dir: ctx.agent_dir,
        agent_db_dir: ctx.agent_db_dir,
        agent_name: ctx.agent_name,
        ssh_config_path: ctx.ssh_config_path,
        resolved_sandbox: ctx.resolved_sandbox,
        internal_client: ctx.internal_client,
        model: probe_writer_model,
        debug_flag: ctx.debug_flag,
        session_locks: ctx.session_locks,
        chat_id: anchor.chat_id,
        thread_id: anchor.thread_id,
        incoming_hint: hint,
    };
    crate::learning_probe_writer::run(writer_ctx, anchor, skill_index).await;
}

/// Record a `learning_skip(reason='budget')` row. Moved verbatim from worker.
pub(crate) async fn record_budget_skip(
    conn: &right_db::Connection,
    agent_name: &str,
    chat_id: i64,
    thread_id: i64,
) {
    if let Err(e) = right_agent::usage::insert::insert_learning_skip(
        conn, "budget", None, Some(chat_id), Some(thread_id),
    )
    .await
    {
        tracing::warn!(agent = %agent_name, "learning_skip insert failed: {e:#}");
    }
}

/// First non-empty line of a skill excerpt (truncated to 200 chars), for the
/// one-line index summary. Moved verbatim from worker.
pub(crate) fn summary_first_line(excerpt: &str) -> String {
    excerpt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
        .unwrap_or_default()
}
```

> Verified against `worker.rs:1073` (200-char truncation preserved). Confirm it still matches before deleting the original in Task 2.

- [ ] **Step 2: Register the module in `lib.rs`.**

In `crates/bot/src/lib.rs`, alongside the other learning module decls (near `pub(crate) mod learning_prefilter;`), add:

```rust
pub(crate) mod learning_pipeline;
```

- [ ] **Step 3: Compile the new module against the rest of the crate.**

Run: `devenv shell -- cargo check -p bot`
Expected: PASS. (The worker still has its own copy of the block + helpers at this point — duplicate private helpers in different modules are fine; the worker copies are still in `worker.rs`. If the compiler complains about an unused `summary_first_line`/`record_budget_skip` in the new module, ignore — Task 2 wires the worker to call into this module.)

- [ ] **Step 4: Commit.**

```bash
git add crates/bot/src/learning_pipeline.rs crates/bot/src/lib.rs
git commit -m "refactor(learning): extract shared post-turn pipeline module"
```

---

## Task 2: Rewire the worker to use the shared pipeline

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (replace `2128-2272`; delete `record_budget_skip` at `425`, `summary_first_line` at `1073`; fix test at `5437`)

- [ ] **Step 1: Replace the inline block with a `run_post_turn` call.**

In `worker.rs`, replace the entire block at lines `2128-2272` (from the `// Post-turn learning pipeline` comment through the closing `}` of the `if let Some(anchor) = ...` guard) with:

```rust
            // Post-turn learning pipeline (prefilter → probe-writer). Fire-and-forget;
            // never blocks user-visible latency. Foreground gate: Normal turns only.
            if let Some(anchor) = post_turn_probe_anchor.take()
                && ctx.learning.prefilter_enabled
                && matches!(cc_prompt_mode, Some(crate::cc::prompt::PromptMode::Normal))
            {
                let learn_ctx = crate::learning_pipeline::PostTurnLearningCtx {
                    agent_dir: ctx.agent_dir.clone(),
                    agent_db_dir: ctx.agent_db_dir.clone(),
                    agent_name: ctx.agent_name.clone(),
                    ssh_config_path: ctx.ssh_config_path.clone(),
                    resolved_sandbox: ctx.resolved_sandbox.clone(),
                    internal_client: Arc::clone(&ctx.internal_client),
                    session_locks: ctx.session_locks.clone(),
                    debug_flag: Arc::clone(&ctx.debug),
                    prefilter_model: ctx
                        .learning
                        .prefilter_model
                        .clone()
                        .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_owned()),
                    probe_writer_enabled: ctx.learning.probe_writer_enabled,
                    probe_writer_model_override: ctx.learning.probe_writer_model.clone(),
                    probe_writer_model_fallback: (**ctx.model.load()).clone(),
                    daily_budget: ctx.learning.max_daily_budget_usd,
                    baseline_window_days: ctx.learning.baseline_window_days,
                    baseline_min_sample: ctx.learning.baseline_min_sample,
                };
                tokio::spawn(async move {
                    crate::learning_pipeline::run_post_turn(learn_ctx, anchor).await;
                });
            }
```

- [ ] **Step 2: Delete the moved helpers from `worker.rs`.**

Delete `async fn record_budget_skip(...)` (was `425-...`) and `fn summary_first_line(...)` (was `1073-...`) from `worker.rs`. They now live in `learning_pipeline`.

- [ ] **Step 3: Fix the worker test that referenced `record_budget_skip`.**

At `worker.rs:5437`, change the call to use the new path:

```rust
        crate::learning_pipeline::record_budget_skip(&conn, "agent-x", 99, 0).await;
```

- [ ] **Step 4: Compile and run the worker/learning tests.**

Run: `devenv shell -- cargo test -p bot learning`
Expected: PASS. Then `devenv shell -- cargo check -p bot` to confirm no dead-code warnings for the now-removed helpers.
Expected: PASS with no `unused function` warnings in `worker.rs`.

- [ ] **Step 5: Commit.**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(learning): worker uses shared run_post_turn"
```

---

## Task 3: Thread learning config + session_locks into cron

**Files:**
- Modify: `crates/bot/src/lib.rs` (reorder `session_locks`; pass into `run_cron_task`)
- Modify: `crates/bot/src/cron.rs` (`run_cron_task` `1467`, `run_job_loop` `1890`, `execute_job` `462` signatures + call sites `1688/1863/1960`)

- [ ] **Step 1: Reorder `session_locks` creation above the cron spawn in `lib.rs`.**

In `crates/bot/src/lib.rs`, move the `let session_locks: crate::telegram::SessionLocks = Arc::new(dashmap::DashMap::new());` line (currently `954`) to **above** the cron spawn block (currently starts `920`). Place it right after the comment at `920` is fine, as long as it precedes `let cron_handle = tokio::spawn(...)`. Leave the sweeper task (`962-979`) where it is.

- [ ] **Step 2: Pass learning config + session_locks into `run_cron_task` at the `lib.rs` call site.**

Before the cron spawn, add:

```rust
    let cron_learning = config.learning.clone();
    let cron_session_locks = Arc::clone(&session_locks);
```

Then extend the `cron::run_cron_task(...)` call (`932-943`) with the two new trailing args:

```rust
        cron::run_cron_task(
            cron_agent_dir,
            cron_agent_name,
            cron_model,
            cron_ssh_config,
            cron_internal_client,
            cron_shutdown,
            cron_sandbox,
            cron_upgrade_lock,
            cron_debug,
            cron_learning,
            cron_session_locks,
        )
```

- [ ] **Step 3: Extend `run_cron_task` signature and thread the values down.**

In `cron.rs`, add two params to `run_cron_task` (`1467`):

```rust
    learning: right_agent_config::LearningConfig,
    session_locks: crate::telegram::SessionLocks,
```

Thread them through to `run_job_loop` (`1890`) and the three `execute_job` call sites (`1688`, `1863`, `1960`). `run_job_loop` gains the same two params; pass `learning.clone()` / `Arc::clone(&session_locks)` into each `execute_job` call. Use `&learning` / `&session_locks` references in the signatures where the existing `model: &Arc<...>` pattern uses references, to avoid clones per tick:

- `run_job_loop(...)` → add `learning: &right_agent_config::LearningConfig, session_locks: &crate::telegram::SessionLocks,`
- `execute_job(...)` → add `learning: &right_agent_config::LearningConfig, session_locks: &crate::telegram::SessionLocks,` as the final params (after `debug`).

> `LearningConfig` lives in `crates/right-agent-config/src/lib.rs`; the bot's `Cargo.toml` depends on `right-agent-config` (crate path `right_agent_config`) — confirmed. Match the existing usage alias (`config.learning` in `lib.rs` is a `right_agent_config::LearningConfig`).

**Also change `execute_job`'s `internal_client` param type** from
`&right_mcp::internal_client::InternalClient` to
`&Arc<right_mcp::internal_client::InternalClient>`. `InternalClient` is **not**
`Clone` (it wraps a single `PathBuf`), so the pipeline must receive an `Arc`
clone — and `run_cron_task` already holds `Arc<InternalClient>`. All three
`execute_job` call sites pass `&ic` where `ic: Arc<InternalClient>`, which is
already `&Arc<...>`; existing `internal_client.<method>()` uses inside
`execute_job` keep working via auto-deref. Where `execute_job` forwards it to a
`&InternalClient`-expecting callee, pass `internal_client` (deref coercion) or
`&**internal_client`.

- [ ] **Step 4: Compile (signatures only — anchor capture comes in Task 4).**

Run: `devenv shell -- cargo check -p bot`
Expected: PASS. `learning` / `session_locks` are now in `execute_job` scope but unused — add `let _ = (&learning, &session_locks);` temporarily at the top of `execute_job` if the unused-variable warning is denied by lints, to be removed in Task 4.

- [ ] **Step 5: Commit.**

```bash
git add crates/bot/src/lib.rs crates/bot/src/cron.rs
git commit -m "refactor(cron): thread learning config + session_locks into execute_job"
```

---

## Task 4: Capture a cron anchor on recurring success and run the pipeline

**Files:**
- Modify: `crates/bot/src/cron.rs` (add `parse_result_stats`; hook in the Success `Ok(delivery_status)` branch)

- [ ] **Step 1: Write the failing test for `parse_result_stats`.**

Add to the `#[cfg(test)] mod tests` in `cron.rs`:

```rust
    #[test]
    fn parse_result_stats_reads_text_turns_cost() {
        let lines = vec![
            r#"{"type":"assistant","message":{}}"#.to_string(),
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done: 3 PRs","num_turns":7,"total_cost_usd":0.34}"#.to_string(),
        ];
        let (text, turns, cost) = super::parse_result_stats(&lines).expect("stats");
        assert_eq!(text, "done: 3 PRs");
        assert_eq!(turns, 7);
        assert!((cost - 0.34).abs() < 1e-9);
    }

    #[test]
    fn parse_result_stats_none_without_result_line() {
        let lines = vec![r#"{"type":"assistant","message":{}}"#.to_string()];
        assert!(super::parse_result_stats(&lines).is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails.**

Run: `devenv shell -- cargo test -p bot parse_result_stats`
Expected: FAIL — `parse_result_stats` not found.

- [ ] **Step 3: Implement `parse_result_stats`.**

Add near `find_last_result_line` (`cron.rs:1156`):

```rust
/// Extract `(result_text, num_turns, total_cost_usd)` from the terminal
/// `{"type":"result"}` line. `None` if there is no result line. Missing
/// `num_turns`/`total_cost_usd` default to 0 (a successful cheap run always
/// carries both, but we never want anchor capture to panic on a partial line).
fn parse_result_stats(lines: &[String]) -> Option<(String, u32, f64)> {
    let line = find_last_result_line(lines)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let text = v.get("result").and_then(|r| r.as_str()).unwrap_or("").to_owned();
    let turns = v.get("num_turns").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
    let cost = v.get("total_cost_usd").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    Some((text, turns, cost))
}
```

- [ ] **Step 4: Run the test to verify it passes.**

Run: `devenv shell -- cargo test -p bot parse_result_stats`
Expected: PASS.

- [ ] **Step 5: Add the `is_recurring` gate test.**

The anchor must be built only for `Recurring`. Add a small pure predicate and test it:

```rust
/// Recurring crons are the only kind whose runs feed skill learning — one-shot
/// `Immediate`/`RunAt` runs never repeat, so a learned skill cannot amortize.
fn schedule_kind_feeds_learning(kind: &right_agent::cron_spec::ScheduleKind) -> bool {
    matches!(kind, right_agent::cron_spec::ScheduleKind::Recurring(_))
}
```

Test:

```rust
    #[test]
    fn only_recurring_feeds_learning() {
        use right_agent::cron_spec::ScheduleKind;
        assert!(super::schedule_kind_feeds_learning(&ScheduleKind::Recurring("*/5 * * * *".into())));
        assert!(!super::schedule_kind_feeds_learning(&ScheduleKind::Immediate));
        assert!(!super::schedule_kind_feeds_learning(&ScheduleKind::RunAt(
            chrono::Utc::now()
        )));
    }
```

Run: `devenv shell -- cargo test -p bot only_recurring_feeds_learning`
Expected: FAIL (predicate missing) → add the predicate → PASS.

- [ ] **Step 6: Capture `started` timing for `wall_elapsed_ms`.**

In `execute_job`, immediately before `let mut child = ... ProcessGroupChild::spawn(cmd)` (currently `cron.rs:~707`), add:

```rust
    let run_started = tokio::time::Instant::now();
```

- [ ] **Step 7: Build the anchor and run the pipeline in the success branch.**

In the `Ok(delivery_status)` arm of the `tx_result` match (the success persist, `cron.rs:~945`), after the `tracing::info!(... "cron output persisted to DB")`, add:

```rust
                                // Skill learning: recurring cron runs feed the
                                // shared pipeline (prefilter → probe-writer fork
                                // of this run's session). Fire-and-forget; never
                                // affects delivery or the run record.
                                if learning.prefilter_enabled
                                    && schedule_kind_feeds_learning(&spec.schedule_kind)
                                    && let Some((reply_text, num_turns, cost_usd)) =
                                        parse_result_stats(&collected_lines)
                                {
                                    let anchor = crate::telegram::worker::ProbeAnchor {
                                        user_msg_text: spec.prompt.clone(),
                                        assistant_reply_text: reply_text,
                                        main_session_uuid: run_id.clone(),
                                        captured_at: chrono::Utc::now(),
                                        chat_id: spec.target_chat_id.unwrap_or(0),
                                        thread_id: spec.target_thread_id.unwrap_or(0),
                                        num_turns,
                                        total_cost_usd: cost_usd,
                                        wall_elapsed_ms: run_started.elapsed().as_millis() as u64,
                                        used_skill_receipts: Vec::new(),
                                    };
                                    let learn_ctx = crate::learning_pipeline::PostTurnLearningCtx {
                                        agent_dir: agent_dir.to_path_buf(),
                                        agent_db_dir: agent_dir.to_path_buf(),
                                        agent_name: agent_name.to_owned(),
                                        ssh_config_path: ssh_config_path.map(|p| p.to_path_buf()),
                                        resolved_sandbox: resolved_sandbox.map(|s| s.to_owned()),
                                        internal_client: Arc::clone(internal_client),
                                        session_locks: session_locks.clone(),
                                        debug_flag: Arc::clone(&debug),
                                        prefilter_model: learning
                                            .prefilter_model
                                            .clone()
                                            .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_owned()),
                                        probe_writer_enabled: learning.probe_writer_enabled,
                                        probe_writer_model_override: learning.probe_writer_model.clone(),
                                        probe_writer_model_fallback: model.map(|s| s.to_owned()),
                                        daily_budget: learning.max_daily_budget_usd,
                                        baseline_window_days: learning.baseline_window_days,
                                        baseline_min_sample: learning.baseline_min_sample,
                                    };
                                    tokio::spawn(async move {
                                        crate::learning_pipeline::run_post_turn(learn_ctx, anchor)
                                            .await;
                                    });
                                }
```

> **Type notes (resolved in Task 3):** `internal_client` is now
> `&Arc<InternalClient>` in `execute_job`, so `Arc::clone(internal_client)`
> above is correct (`InternalClient` is not `Clone`). `ssh_config_path` /
> `resolved_sandbox` / `agent_dir` / `agent_name` / `model` are borrowed params;
> the `.to_path_buf()` / `.map(str::to_owned)` conversions above produce the
> owned forms the `move` closure needs.

- [ ] **Step 8: Remove the temporary `let _ = ...` from Task 3 Step 4 if added.**

- [ ] **Step 9: Compile and run cron tests.**

Run: `devenv shell -- cargo test -p bot cron`
Expected: PASS (including the two new `parse_result_stats` tests and `only_recurring_feeds_learning`).

- [ ] **Step 10: Commit.**

```bash
git add crates/bot/src/cron.rs
git commit -m "feat(cron): recurring runs feed the skill-learning pipeline"
```

---

## Task 5: Budget-skip integration test for the cron path

**Files:**
- Modify: `crates/bot/src/learning_pipeline.rs` (add a `#[cfg(test)]` module)

This proves the shared gate writes a `learning_skip` row when the budget is exhausted, independent of which caller built the anchor.

- [ ] **Step 1: Write the failing test.**

Add to `learning_pipeline.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn budget_exhausted_writes_skip_row() {
        // In-memory DB with the learning_skip table available via migrations.
        let conn = right_db::open_connection(":memory:", true).await.expect("db");
        record_budget_skip(&conn, "agent-x", 42, 7).await;
        let n = right_agent::usage::query::count_learning_skips(&conn, "budget")
            .await
            .expect("count");
        assert_eq!(n, 1);
    }
}
```

> Verify the helper name `count_learning_skips` against `right_agent::usage::query`. If no such counter exists, query directly with `right_db` (`SELECT COUNT(*) FROM learning_skip WHERE reason = 'budget'`) using the crate's existing query pattern — match how `worker.rs` tests at `5437` assert the skip row, and reuse that exact assertion helper.

- [ ] **Step 2: Run to verify it fails, then passes.**

Run: `devenv shell -- cargo test -p bot budget_exhausted_writes_skip_row`
Expected: FAIL (if the assertion helper name is wrong) → fix to match the existing pattern from the worker test at `5437` → PASS.

- [ ] **Step 3: Commit.**

```bash
git add crates/bot/src/learning_pipeline.rs
git commit -m "test(learning): budget-skip row written by shared pipeline"
```

---

## Task 6: Live end-to-end test (CI-explicit, ignored locally per repo convention)

**Files:**
- Create: `crates/bot/tests/ci_cron_learning.rs`

Per `AGENTS.rust.md` §5 and ARCHITECTURE.md, tests needing a live sandbox + Claude use `#[ignore = "ci-claude: ..."]` with a `ci_claude_` name prefix and are invoked by `.github/workflows/tests.yml`. This one verifies risk (a) from the spec: a just-finished cron session is forkable inside the sandbox and a `CreateNew` decision lands a `rightx-*` skill.

- [ ] **Step 1: Write the ignored live test.**

```rust
//! Live: a recurring cron run that demonstrates a reusable procedure produces a
//! non-Skip prefilter decision and the probe-writer forks the run's session to
//! create a rightx-* skill. Verifies the cron→learning path end to end.

#[tokio::test]
#[ignore = "ci-claude: requires live sandbox + Claude + OAuth token"]
async fn ci_claude_recurring_cron_creates_skill() {
    // Arrange: a TestSandbox agent with a recurring cron spec whose prompt is a
    // clearly codifiable procedure, learning.prefilter_enabled = true, and a
    // funded daily budget. Run one cron tick via execute_job, then assert a
    // rightx-* skill appears in the sandbox skill index within a bounded wait.
    //
    // Use right_openshell::test_support::TestSandbox::create(...) (never the
    // openshell CLO, never a hardcoded sandbox name — see ARCHITECTURE.md
    // "Integration Tests Using Live Sandboxes").
    todo!("implement against TestSandbox once the unit path is green");
}
```

> This is the ONE place a `todo!()` is acceptable in this plan: the live harness wiring depends on the `TestSandbox` builder surface, which the implementer should read at build time (`crates/right-openshell/src/test_support.rs`). Replace the `todo!()` with the real arrange/act/assert before marking the task done. Do NOT add `#[ignore]` to any non-live test.

- [ ] **Step 2: Confirm it compiles and is collected as ignored.**

Run: `devenv shell -- cargo test -p bot ci_claude_recurring_cron_creates_skill -- --list`
Expected: the test is listed. Do not run it locally (no `--ignored`).

- [ ] **Step 3: Wire it into the CI ignored-test job if not auto-collected.**

Check `.github/workflows/tests.yml` for the `ci_claude_` filter; the existing workspace-wide ignored filter should pick it up by prefix. Confirm no per-file allowlist needs editing (`crates/right/tests/ci_ignored_contract.rs` enforces the prefix/reason convention — run it).

Run: `devenv shell -- cargo test -p right ci_ignored_contract`
Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add crates/bot/tests/ci_cron_learning.rs
git commit -m "test(cron): live ci-claude cron-learning skill-creation gate"
```

---

## Task 7: Docs + final workspace verification

**Files:**
- Modify: `docs/architecture/learning.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update `docs/architecture/learning.md` gate-ordering section.**

In the "Gate ordering" section (§1, the prefilter + probe-writer gate), add a paragraph noting the second trigger source:

```markdown
The same gate (budget check → prefilter → probe-writer) now runs from two
call sites via the shared `bot::learning_pipeline::run_post_turn`. The
foreground worker invokes it for `PromptMode::Normal` turns; cron
(`bot::cron::execute_job`) invokes it after a **recurring** run
(`ScheduleKind::Recurring`) succeeds, building a `ProbeAnchor` whose
`main_session_uuid` is the cron run's session id so the probe-writer forks
that run's transcript. One-shot (`Immediate`/`RunAt`) cron runs are excluded.
Cron uses the foreground 14d baselines as-is (v1 approximation — cron runs
read as above-baseline; revisit with cron-specific baselines only if the
prefilter over-triggers).
```

- [ ] **Step 2: Update the `ARCHITECTURE.md` skill-learning contract.**

In the "Skill learning" section, append one sentence after the `LEARNING_SOURCES` paragraph:

```markdown
The per-turn pipeline runs from two call sites through the shared
`bot::learning_pipeline::run_post_turn`: foreground `Normal` turns and
recurring-cron successes (`ScheduleKind::Recurring`; one-shot cron runs are
excluded). No new `LEARNING_SOURCES` entry — cron learning *is*
`learning_prefilter` + `learning_probe_writer` spend.
```

Verify ARCHITECTURE.md stays under the 40k-char hard budget (`wc -c ARCHITECTURE.md`); if the addition would push it over, trim elsewhere in the same commit per `AGENTS.md`.

- [ ] **Step 3: Final full workspace test (mandatory).**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Re-run any flaky failures in isolation per `project_flaky_tests_parallel_load` before attributing them to this change.

- [ ] **Step 4: Commit + land.**

```bash
git add docs/architecture/learning.md ARCHITECTURE.md
git commit -m "docs(learning): document cron recurring as a learning trigger"
```

Land via fast-forward push to `origin/master` from the worktree (project convention), then clean up the worktree.

---

## Self-review notes (for the executor)

- **Spec coverage:** Task 1-2 = shared extraction; Task 3-4 = cron wiring + recurring gate + anchor (spec §1-4); Task 4 Step 7 = v1 baseline reuse (spec §5, inherited from `run_post_turn`); Task 6 = spec risk (a); Task 7 = doc updates. Spec risk (b) (`execute_job` scope) is resolved inline in Task 4 (`spec.target_chat_id/thread_id`, `run_started`).
- **No new `LEARNING_SOURCES`** — confirmed: cron reuses `learning_prefilter`/`learning_probe_writer`; the dashboard sources test needs no change.
- **Resolved during planning:** `InternalClient` is not `Clone` → `execute_job` takes `&Arc<InternalClient>` (Task 3); `summary_first_line` truncates to 200 chars (Task 1); `right_agent_config::LearningConfig` is the correct path (Task 3).
- **One open verification point** (do not skip): `count_learning_skips` helper name in `right_agent::usage::query` (Task 5 Step 1) — if absent, mirror the assertion pattern from the existing worker test at `worker.rs:5437`.
