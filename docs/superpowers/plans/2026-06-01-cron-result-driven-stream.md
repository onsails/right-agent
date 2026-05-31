# Result-driven cron stream + doctor visibility — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop cron stream reads from hanging when CC finishes but the SSH stdout never reaches EOF, deliver the real notification on success, and surface in-flight cron runs in `right doctor`.

**Architecture:** `consume_cron_stream` breaks on the terminal top-level `result` event instead of waiting for EOF; `execute_job` routes success/failure off the `CronStreamOutcome` (the result event) rather than the subprocess exit code; a new doctor check lists `async_runs` rows still `status='running'`. No wall-clock deadline (the existing 60 s shutdown-drain is the backstop for the rare no-result tail).

**Tech Stack:** Rust 2024, tokio, `right_process::ProcessGroupChild`, `right_db` (turso), `right_agent::doctor`, `right_ui`.

**Scope note:** This plan covers spec Parts 1, 2, and 3a (doctor). Spec Part 3b (dashboard overview list) is an independent frontend subsystem (Vue + `right-dashboard` read-model + SSR tests) and gets its own follow-up plan. The spec is at `docs/superpowers/specs/2026-06-01-cron-result-driven-stream-design.md`.

---

## File structure

- `crates/bot/src/cron.rs` — **modify**
  - `CronStreamOutcome` enum (~1099): drop `TimedOut`, drop `Failed::exit_code`, drop `#[allow(dead_code)]`.
  - `consume_cron_stream` (~1130): drop `deadline` param; break on terminal result; bounded by result/EOF only.
  - new private helper `terminal_result_line(line: &str) -> Option<String>`.
  - `execute_job` post-break + status gate (~723–840): make `child.wait()` non-fatal, derive `exit_code: Option<i32>`, route on the outcome.
  - `CRON_STREAM_DEADLINE_SECS` const (~1221): delete (unused after Part 1).
  - new unit tests + update the existing `ci_openshell_cron_stream_survives_wedged_stdout` call site.
- `crates/right-agent/src/doctor.rs` — **modify**: new `check_cron_runs(agent_dir) -> Vec<DoctorCheck>`, wire into `run_doctor`'s per-agent loop.
- `crates/right-agent/src/doctor_tests.rs` — **modify**: test for `check_cron_runs`.

---

## Task 1: `consume_cron_stream` breaks on the terminal result event

**Files:**
- Modify: `crates/bot/src/cron.rs` (enum ~1099, fn ~1130, const ~1221, repro test ~2546)
- Test: `crates/bot/src/cron.rs` (new `#[cfg(test)]` unit tests near the existing cron tests)

- [ ] **Step 1: Write the failing unit tests**

Add these to the existing `mod target_snapshot_tests` (or the nearest `#[cfg(test)] mod` that has `use super::*;`) in `crates/bot/src/cron.rs`. They use a local `bash` child (no sandbox) that reproduces "result emitted, stdout stays open" exactly like the SSH wedge:

```rust
#[tokio::test]
async fn consume_cron_stream_breaks_on_terminal_result_without_eof() {
    use std::time::Duration;
    // Prints a terminal result line, then holds stdout open (no EOF) via sleep.
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c").arg(
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"UNIT-OK"}'; sleep 30"#,
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");

    let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
        .await
        .expect("consume_cron_stream must return without waiting for EOF");

    match outcome {
        CronStreamOutcome::Success { result_line, .. } => {
            assert!(result_line.contains("UNIT-OK"), "got: {result_line}");
        }
        other => panic!("expected Success, got {other:?}"),
    }
}

#[tokio::test]
async fn consume_cron_stream_eof_without_result_is_failed() {
    use std::time::Duration;
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(r#"printf '%s\n' '{"type":"assistant","message":{"content":[]}}'"#);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");

    let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
        .await
        .expect("must return at EOF");
    assert!(
        matches!(outcome, CronStreamOutcome::Failed { .. }),
        "EOF without a terminal result must be Failed, got {outcome:?}"
    );
}

#[tokio::test]
async fn consume_cron_stream_nested_result_is_not_terminal() {
    use std::time::Duration;
    // A result-shaped line carrying parent_tool_use_id is a sub-agent result,
    // NOT the terminal top-level result; EOF then yields Failed.
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c").arg(
        r#"printf '%s\n' '{"type":"result","parent_tool_use_id":"toolu_x","result":"NESTED"}'"#,
    );
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = right_process::ProcessGroupChild::spawn(cmd).expect("spawn bash");

    let outcome = tokio::time::timeout(Duration::from_secs(5), consume_cron_stream(&mut child))
        .await
        .expect("must return at EOF");
    assert!(
        matches!(outcome, CronStreamOutcome::Failed { .. }),
        "nested result must not be treated as terminal, got {outcome:?}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot consume_cron_stream -- --nocapture`
Expected: compile error first (`consume_cron_stream` still takes a `deadline` arg; `CronStreamOutcome` still has `TimedOut`). After fixing the call to match the *current* 2-arg-less form they will still FAIL/await — but do NOT adjust signatures here; the point is the next step changes the function. If they compile against the current signature, `consume_cron_stream_breaks_on_terminal_result_without_eof` FAILS by hanging to the 5 s timeout (the unbounded loop never sees EOF). That is the RED.

> Note: the current signature is `consume_cron_stream(child, deadline)`. The tests above call it with one arg on purpose — they will not compile until Step 3 drops the `deadline` param. A non-compiling RED is acceptable; the behavioral RED (the hang) is proven by the existing `ci_openshell_` test and reproduced locally once Step 3's signature lands but the body is still unbounded. Implement Step 3 in one shot.

- [ ] **Step 3: Rewrite the enum, helper, and function body**

In `crates/bot/src/cron.rs`, replace the `CronStreamOutcome` enum (currently ~1099–1119) with:

```rust
/// Outcome of consuming the cron CC subprocess stdout stream.
///
/// `collected_lines` carries every NDJSON line read from stdout so the caller
/// can run [`parse_cron_output`].
#[derive(Debug)]
pub(crate) enum CronStreamOutcome {
    /// A terminal top-level `{"type":"result"}` event was observed (the loop
    /// broke on it, or it was found at EOF). `result_line` is its raw JSON.
    Success {
        result_line: String,
        collected_lines: Vec<String>,
    },
    /// Stdout reached EOF without a terminal `result` event.
    Failed { collected_lines: Vec<String> },
}
```

Add this helper directly above `consume_cron_stream`:

```rust
/// Return the line iff it is the terminal top-level CC result event.
///
/// CC emits exactly one top-level `{"type":"result"}` summary at the end of a
/// turn. Sub-agent (Task tool) results arrive as nested `assistant`/`user`
/// messages carrying `parent_tool_use_id`, so `type == "result"` is already
/// terminal; the `parent_tool_use_id` absent/null check is defense-in-depth.
fn terminal_result_line(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let is_result = v.get("type").and_then(|t| t.as_str()) == Some("result");
    let top_level = v.get("parent_tool_use_id").map(|p| p.is_null()).unwrap_or(true);
    (is_result && top_level).then(|| line.to_string())
}
```

Replace `consume_cron_stream` (currently ~1130–1164) with:

```rust
/// Consume the cron CC subprocess stdout line-by-line and classify the outcome.
///
/// Breaks immediately on the terminal top-level `result` event (does NOT wait
/// for EOF — the SSH stdout pipe can linger open after CC exits). On EOF,
/// returns `Success` iff a terminal result was seen, else `Failed`. There is no
/// wall-clock bound here; a turn that never emits a result is bounded by the
/// shutdown-drain (`SHUTDOWN_JOB_TIMEOUT`).
pub(crate) async fn consume_cron_stream(
    child: &mut right_process::ProcessGroupChild,
) -> CronStreamOutcome {
    let stdout = child.stdout().expect("stdout piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();

    let mut collected_lines: Vec<String> = Vec::new();
    let mut result_line: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if result_line.is_none() {
            result_line = terminal_result_line(&line);
        }
        collected_lines.push(line);
        if result_line.is_some() {
            break; // terminal result seen — do not wait for EOF
        }
    }

    match result_line {
        Some(result_line) => CronStreamOutcome::Success {
            result_line,
            collected_lines,
        },
        None => CronStreamOutcome::Failed { collected_lines },
    }
}
```

Delete the `CRON_STREAM_DEADLINE_SECS` constant (currently ~1218–1221) — it is now unused.

- [ ] **Step 4: Update the `execute_job` call site to the new signature**

In `crates/bot/src/cron.rs`, the call site (currently ~705–717) destructures three variants and constructs a deadline. Replace the deadline construction + match with:

```rust
    // Stream stdout; break on the terminal result event (do not wait for EOF —
    // the SSH stdout pipe can linger open after CC exits).
    let outcome = consume_cron_stream(&mut child).await;
    let collected_lines: Vec<String> = match &outcome {
        CronStreamOutcome::Success {
            collected_lines, ..
        }
        | CronStreamOutcome::Failed { collected_lines } => collected_lines.clone(),
    };
```

(`outcome` is consumed by the gate in Task 2; `collected_lines` is cloned here so the post-break/log code between still compiles. Task 2 removes this clone when it folds the bodies into the match.)

- [ ] **Step 5: Update the existing `ci_openshell_` repro test call**

In the repro test (`ci_openshell_cron_stream_survives_wedged_stdout`, ~2546–2549), drop the now-removed `deadline` argument:

```rust
        let outcome =
            tokio::time::timeout(Duration::from_secs(25), consume_cron_stream(&mut child))
                .await;
```

Delete the now-unused `let deadline = tokio::time::Instant::now() + Duration::from_secs(10);` line above it. Leave the rest of that test unchanged.

- [ ] **Step 6: Run the unit tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot consume_cron_stream -- --nocapture`
Expected: PASS (3 new tests). `cargo check -p right-bot --tests` compiles.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "fix(cron): break cron stream on terminal result event, drop deadline placeholder"
```

---

## Task 2: `execute_job` routes success/failure off the outcome, not the exit code

**Files:**
- Modify: `crates/bot/src/cron.rs:723-840` (post-break cleanup + status gate)

- [ ] **Step 1: Make `child.wait()` non-fatal and derive `exit_code: Option<i32>`**

In `crates/bot/src/cron.rs`, the post-break block (currently ~726–752) builds `exit_status: Option<ExitStatus>` then early-returns on `None` (lines ~747–751) and unwraps (line ~752). A wedged child (Success delivered via break) must NOT bail. Replace lines ~747–759 (the `if exit_status.is_none() { ... return; }`, the `let exit_status = exit_status.unwrap();`, and the following debug log) with:

```rust
    // Best-effort only: the outcome (Task 1) decides success/failure, not the
    // exit code. A wedged transport leaves `exit_status` None; we still deliver
    // a Success outcome. ProcessGroupChild::Drop killpg's the group on return.
    let exit_code: Option<i32> = exit_status.and_then(|s| s.code());
    tracing::debug!(
        job = %job_name,
        child_pid,
        exit_code = ?exit_code,
        wait_ms = wait_started.elapsed().as_millis() as u64,
        "post-break: child wait completed (best-effort)",
    );
```

Leave the `child.wait()` timeout match (726–746) and the stderr drain (763–794) unchanged.

- [ ] **Step 2: Replace the exit-code status with an outcome-derived status for logging**

Replace the status computation (currently ~796–807, `let exit_code = exit_status.code();` through the `let status = if exit_status.success() { ... }` block) with:

```rust
    let status = match &outcome {
        CronStreamOutcome::Success { .. } => "success",
        CronStreamOutcome::Failed { .. } => "failed",
    };
    if matches!(outcome, CronStreamOutcome::Failed { .. }) {
        tracing::error!(job = %job_name, exit_code = ?exit_code, "cron job produced no terminal result");
    }
```

(`exit_code` is now the `Option<i32>` from Step 1. The `status` log at ~834 keeps working.)

- [ ] **Step 3: Route the success/failure branch on the outcome**

The gate is currently `if exit_status.success() { <success body> } else { <failure body> }` (~837 / ~975). Change ONLY the branch heads; the bodies stay byte-for-byte.

Change the success head (line ~837) from:

```rust
    if exit_status.success() {
        match parse_cron_output(&collected_lines) {
```

to:

```rust
    match outcome {
        CronStreamOutcome::Success { collected_lines, .. } => {
        match parse_cron_output(&collected_lines) {
```

Change the failure head (line ~975) from:

```rust
    } else {
        // Failure path: commit terminal status='failed' before reflection runs.
```

to:

```rust
        }
        CronStreamOutcome::Failed { collected_lines } => {
        // Failure path: commit terminal status='failed' before reflection runs.
```

Then add one extra closing brace at the end of the failure body to close the `match outcome` (where the old `else` block closed). Verify brace balance with the compiler in Step 4.

Because both arms now bind their own `collected_lines`, delete the outer `let collected_lines = match &outcome { ... }.clone();` added in Task 1 Step 4 (it is now shadowed/unused). The failure body's references to `collected_lines`, `stderr_str`, `exit_code`, `spec`, `find_last_result_line`, etc. resolve against the arm binding and outer scope unchanged.

- [ ] **Step 4: Compile and run the cron unit + ignored repro test**

Run: `devenv shell -- cargo test -p right-bot cron:: -- --nocapture`
Expected: PASS (existing 38 cron unit tests + Task 1's 3). Fix any brace/borrow errors surfaced by the compiler.

Run the end-to-end repro (real sandbox; dev machine has OpenShell) to confirm it flipped RED→GREEN:
Run: `devenv shell -- cargo test -p right-bot ci_openshell_cron_stream -- --ignored --nocapture`
Expected: PASS — `consume_cron_stream` returns `Success` carrying `REPRO-OK` well within the 25 s outer timeout (previously hung).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cron.rs
git commit -m "fix(cron): deliver cron result from terminal event regardless of subprocess exit"
```

---

## Task 3: doctor lists in-flight cron runs

**Files:**
- Modify: `crates/right-agent/src/doctor.rs` (new `check_cron_runs`, wire into `run_doctor`)
- Test: `crates/right-agent/src/doctor_tests.rs`

- [ ] **Step 1: Write the failing doctor test**

Add to `crates/right-agent/src/doctor_tests.rs` (mirror the existing `null_target_warns` style — tempdir, migrate=true DB, seed, call, assert):

```rust
#[tokio::test]
async fn check_cron_runs_lists_running_rows() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).await.unwrap();
    // Seed one running cron run and one finished one; only the running one shows.
    conn.execute(
        "INSERT INTO async_runs (id, kind, producer_ref, run_session_id, target_chat_id,
            status, started_at, delivery_required, delivery_status, created_at, updated_at)
         VALUES ('r1','cron','hyperbot-tracker','r1',100,'running','2026-06-01T00:00:00+00:00',
            0,'none','2026-06-01T00:00:00+00:00','2026-06-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO async_runs (id, kind, producer_ref, run_session_id, target_chat_id,
            status, started_at, finished_at, exit_code, delivery_required, delivery_status,
            created_at, updated_at)
         VALUES ('r2','cron','github-tracker','r2',100,'success','2026-06-01T00:00:00+00:00',
            '2026-06-01T00:03:00+00:00',0,0,'none','2026-06-01T00:00:00+00:00','2026-06-01T00:00:00+00:00')",
        (),
    )
    .await
    .unwrap();
    drop(conn);

    let checks = check_cron_runs(dir.path()).await;
    assert_eq!(checks.len(), 1, "only the running run should be listed: {checks:?}");
    assert!(checks[0].detail.contains("hyperbot-tracker"));
    assert!(checks[0].detail.contains("2026-06-01T00:00:00+00:00"));
    assert_eq!(checks[0].status, CheckStatus::Pass); // informational, no auto-verdict
}

#[tokio::test]
async fn check_cron_runs_empty_when_none_running() {
    let dir = tempfile::tempdir().unwrap();
    let conn = right_db::open_connection(dir.path(), true).await.unwrap();
    drop(conn);
    let checks = check_cron_runs(dir.path()).await;
    assert!(checks.is_empty(), "no running runs → no checks: {checks:?}");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `devenv shell -- cargo test -p right-agent check_cron_runs -- --nocapture`
Expected: FAIL — `check_cron_runs` not found (does not compile).

- [ ] **Step 3: Implement `check_cron_runs`**

Add to `crates/right-agent/src/doctor.rs` (near `check_cron_targets`):

```rust
/// List cron runs still `status='running'` in this agent's `data.db`.
///
/// Informational only (no staleness threshold): a long-running row may be a
/// detached sandbox orphan or the rare no-result tail — we show the start time
/// and let the operator judge. Emits nothing when no run is in flight.
pub async fn check_cron_runs(agent_dir: &Path) -> Vec<DoctorCheck> {
    let conn = match right_db::open_connection(agent_dir, false).await {
        Ok(c) => c,
        Err(_) => return Vec::new(), // unreadable DB is covered by check_memory
    };
    let mut stmt = match conn.prepare(
        "SELECT producer_ref, started_at FROM async_runs \
         WHERE kind = 'cron' AND status = 'running' \
         ORDER BY started_at",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        })
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for row in rows {
        let Ok((job, started_at)) = row else { continue };
        let job = job.unwrap_or_else(|| "<unknown>".into());
        let started = started_at.unwrap_or_else(|| "<unknown>".into());
        out.push(DoctorCheck {
            name: "cron run".to_string(),
            status: CheckStatus::Pass,
            detail: format!("{job} running since {started}"),
            fix: None,
        });
    }
    out
}
```

- [ ] **Step 4: Wire it into `run_doctor`'s per-agent loop**

In `crates/right-agent/src/doctor.rs`, in the agent loop where `check_memory` and `check_cron_targets` are called per `data.db`-bearing agent dir (~342–347), add after the `check_cron_targets` call, mirroring its name-prefix pattern:

```rust
        for mut chk in check_cron_runs(&path).await {
            chk.name = format!("{name}/{}", chk.name);
            checks.push(chk);
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `devenv shell -- cargo test -p right-agent check_cron_runs -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/right-agent/src/doctor.rs crates/right-agent/src/doctor_tests.rs
git commit -m "feat(doctor): list in-flight cron runs per agent"
```

---

## Final verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing failures unrelated to these changes (the explore noted baseline clippy issues in `right-mcp`/generated proto — not introduced here).

- [ ] **Step 2: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 3: Commit any fixups**, then this plan is complete.

---

## Follow-up (separate plan)

Spec Part 3b — dashboard overview list of running cron runs — is an independent frontend subsystem:
`right-agent::async_runs`/`right-dashboard` read-model query (`WHERE kind='cron' AND status='running'`), a field on `DashboardOverviewResponse` (built in `dashboard_overview`, `crates/right-dashboard/src/read_model/dashboard_overview.rs`), `OverviewView.vue` rendering via `AsyncState.vue`, and SSR + pure-`*.ts` tests. It is shippable on its own and does not block Parts 1–3a. Write it as `docs/superpowers/plans/2026-06-01-cron-dashboard-running-runs.md` next.

---

## Self-review

- **Spec coverage:** Part 1 → Task 1. Part 2 → Task 2. Part 3a (doctor) → Task 3. Part 3b (dashboard) → explicitly deferred to a follow-up plan (scope split). Repro test RED→GREEN → Task 2 Step 4. No-deadline decision → encoded (no deadline branch; `CRON_STREAM_DEADLINE_SECS` deleted; shutdown-drain noted as backstop). Removed dead code (`TimedOut`, `Failed::exit_code`, `#[allow(dead_code)]`) → Task 1 Step 3.
- **Type consistency:** `CronStreamOutcome::Success { result_line, collected_lines }` / `Failed { collected_lines }` used identically in `consume_cron_stream`, the call site (Task 1 Step 4), the gate (Task 2 Step 3), and tests. `consume_cron_stream(&mut child)` (one arg) used consistently in unit tests, repro test, and call site. `check_cron_runs(agent_dir: &Path) -> Vec<DoctorCheck>` matches its test calls and the `run_doctor` wiring. `CheckStatus::Pass` and `DoctorCheck { name, status, detail, fix }` match the real struct from doctor.rs.
- **Placeholder scan:** none — every code step shows full code; every run step shows the command and expected result.
