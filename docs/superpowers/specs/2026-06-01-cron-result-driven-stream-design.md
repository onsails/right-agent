# Result-driven cron stream + stuck-run visibility

**Date:** 2026-06-01
**Status:** Design approved, ready for plan

## Problem

`right down` repeatedly blocks on "waiting for `right` agent to finish cron
hyperbot-tracker", even though the job is scheduled every 6 hours and a
healthy run takes ~3 minutes.

### Root cause (confirmed from logs)

The cron job did not run for hours — it **completed in 2–4 minutes and then
the bot failed to notice**. Every hung run's NDJSON log
(`/sandbox/crons/logs/hyperbot-tracker-<id>.ndjson`) ends with a normal
terminal event:

```json
{"type":"result","subtype":"success","is_error":false,...,"terminal_reason":"completed"}
```

CC generated the result (and the real notification, present in
`structured_output.delivery`), then exited. But the bot's stdout stream loop
in `crates/bot/src/cron.rs::execute_job` is **unbounded**:

```rust
let mut lines = tokio::io::BufReader::new(stdout).lines();
while let Ok(Some(line)) = lines.next_line().await {
    collected_lines.push(line);
}
```

`next_line().await` waits for the next line **or EOF**. Over SSH the stdout
channel sometimes never reaches EOF after the final result line (a lingering
forwarded FD / PTY master holds it open). The final `next_line()` blocks
forever. The `POST_BREAK_*` cleanup timeouts sit *after* the loop and are
never reached; `ProcessGroupChild::Drop` (killpg) only fires on function
return, which never happens. The task parks until shutdown.

The worker (`crates/bot/src/telegram/worker.rs` ~line 3353) wraps the same
`lines.next_line()` in a `tokio::select!` with a `sleep_until(deadline)`
branch, so it is bounded. **Cron is not.** That asymmetry is the bug.

### Evidence

Across 91 `hyperbot-tracker` runs (agent `right`):

| bucket | count | duration |
|---|---|---|
| healthy (`success`, exit 0) | 72 | 34–367 s |
| **hung** (`failed`, NULL exit_code) | 6 | 8,569–375,769 s |
| instant fail (exit 255) | 6 | 0 s |

Every hung run carries
`error_json = {"kind":"cron_shutdown_interrupted","reason":"shutdown timeout"}`
— i.e. `finished_at` is just when `right down` was run, not real work. Two
zombies were reaped by a single shutdown on 2026-05-26 (both finished at
`21:05:49`), proving hung tasks accumulate between shutdowns.

### Collateral damage

Because the bot never parses the terminal `result`, the real tracker
notification is lost and the user instead receives a **failure** delivery
from the reflection path. So this is not only a shutdown nuisance — it
silently drops user-visible tracker updates.

## Goals

1. Stop cron stream reads from hanging on the common "result emitted, EOF
   never arrives" case.
2. Deliver the real notification when CC produced a terminal result, even if
   the transport/process did not exit cleanly.
3. Make any genuinely stuck cron run **visible** (dashboard + doctor) without
   inventing staleness heuristics.

## Non-goals (YAGNI)

- **Wall-clock execution deadline.** Deferred. With break-on-result the
  observed hang class (all 6 had a result) is gone; the only residual is a
  run where `result` never arrives at all (model truly stuck, or transport
  wedges before the result line). That tail is already bounded by the
  existing 60 s `SHUTDOWN_JOB_TIMEOUT` shutdown-drain, which aborts the task
  and killpg's it. We add a deadline later only if the monitor shows the tail
  actually bites. Not overloading `lock_ttl` (a duplicate-spawn guard) with a
  second meaning.
- In-sandbox scan for orphaned `claude` processes.
- Per-job configurable run timeout (`cron_specs` schema change).
- Inactivity (between-lines) timeout — wall-clock isn't even being added; the
  shutdown-drain is the backstop.

## Design

### Part 1 — `consume_cron_stream`: break on the terminal result event

`consume_cron_stream` already exists (extracted, behavior-preserving) in
`crates/bot/src/cron.rs`. Replace its unbounded body:

- Read lines with `tokio::io::BufReader::lines()`, classify each via the
  existing `crate::cc::stream::parse_stream_event` (same parser the worker
  uses — do not hand-roll JSON checks).
- On `StreamEvent::Result(json)` for the **terminal top-level** result
  (`parent_tool_use_id` absent/null), push the line, then **`break` with
  `CronStreamOutcome::Success { result_line, collected_lines }`** — do not
  wait for EOF.
  - Rationale for the top-level guard: CC emits exactly one top-level
    `type:"result"` summary at the very end. Sub-agent (Task tool) results
    arrive as nested assistant/user messages carrying `parent_tool_use_id`,
    not as `type:"result"`, so `type=="result"` is already uniquely
    terminal; the `parent_tool_use_id == null` check is defense-in-depth and
    matches how the worker captures `result_line`.
- On EOF (`Ok(None)`): `Success` if a terminal result was seen earlier,
  else `CronStreamOutcome::Failed { exit_code: None, collected_lines }`.
- **No deadline branch** (deferred). Remove the placeholder `deadline`
  parameter, the `CRON_STREAM_DEADLINE_SECS` constant, and the
  `CronStreamOutcome::TimedOut` variant the extraction added — do not ship
  dead code. `CronStreamOutcome` becomes `Success { result_line,
  collected_lines } | Failed { exit_code, collected_lines }`.

### Part 2 — `execute_job`: derive status from the outcome, not the exit code

Today `execute_job` decides success via `exit_status.success()` (cron.rs
~838), with the failure branch at ~979 and reflection at ~1027. That is the
source of the lost notification: when the transport wedges, `child.wait()`
times out → `update_failed_run_record` → reflection sends a failure, and the
real `result` (already in `collected_lines`) is discarded.

Change the gate to the `CronStreamOutcome`:

- `Success { result_line, collected_lines }` → existing success path:
  `parse_cron_output(&collected_lines)` → `finish_run("success")` → deliver
  the real notification. This runs **regardless** of how/whether the child
  process exits.
- `Failed { collected_lines, .. }` → existing failure path:
  `update_run_record(failed)` + `reflect_on_failure`.

The post-break `child.wait()` (bounded by `POST_BREAK_WAIT_TIMEOUT_SECS`) and
stderr drain stay, but become **best-effort and advisory only**: used to log
the exit code, no longer the success/failure arbiter. Final teardown is still
`ProcessGroupChild::Drop` → `killpg(SIGKILL)` on return, which reaps the local
`ssh` group; dropping the connection makes remote `sshd` SIGHUP the remote
session (the remote `claude` has already exited).

### Part 3 — stuck-run visibility (no auto-verdict)

Signal: rows in each agent's `data.db` with `kind='cron' AND
status='running'`. Post-fix a healthy run resolves in minutes, so a long
`running` row is anomalous (a detached remote orphan, or the rare no-result
tail between shutdowns). We **show**, we do not classify — consistent with
"show the date, let the human judge; no TTL/threshold heuristics in code".

- **doctor** (`crates/right-agent/src/doctor.rs`): add a `check_*` mirroring
  the existing check pattern. It queries `async_runs` for cron rows with
  `status='running'` and reports `job_name`, `started_at`, and age. Neutral
  status (informational list, not a threshold-driven fail/warn). Output via
  `right_ui` atoms like the other checks.
- **dashboard** (`right-dashboard`): surface the same list on the overview
  via **injected runtime state / the internal Unix-socket API** — never by
  running doctor implicitly (dashboard write/overview contract). Render
  through `components/AsyncState.vue`; pure decision logic (if any) extracted
  to a `*.ts` helper and SSR-tested. Show `started_at`/age; no verdict.

Parts 1+2 and Part 3 are independently shippable; the plan phases them.

## Testing

- **RED → GREEN (already written):**
  `ci_openshell_cron_stream_survives_wedged_stdout` (`#[ignore =
  "ci-openshell: creates real sandbox"]`, `ci_openshell_` prefix) drives a
  real sandbox whose remote script prints one canonical `result` line then
  `sleep`s (stdout never EOFs). It asserts `consume_cron_stream` returns
  `Success` carrying the result within a bounded window. RED today (hangs →
  outer timeout); GREEN after Part 1. Its call site updates to the new
  `consume_cron_stream` signature when Part 1 drops the `deadline` parameter.
- **New unit tests (no sandbox, fake child):**
  - `consume_cron_stream` breaks on a terminal `result` line even when more
    lines / no EOF follow → `Success`. Use a local
    `bash -c 'printf "<result>\n"; sleep N'` child via `ProcessGroupChild`
    and an outer `tokio::time::timeout` to bound the assertion.
  - EOF with no result line → `Failed`.
  - A nested result-shaped line carrying `parent_tool_use_id` does **not**
    trigger the terminal break.
- **doctor unit test:** seed an in-memory/temp `data.db` with a `running`
  cron `async_runs` row; assert the new check lists it with job name and
  started_at. Mirror `doctor_tests.rs` style.
- **dashboard:** SSR test (`@vue/server-renderer renderToString`) for the new
  list rendering loading/empty/content via `AsyncState.vue`; unit-test any
  extracted `*.ts` resolver directly.
- **Cadence:** targeted package tests during the red/green loop
  (`cargo test -p right-bot consume_cron_stream`,
  `cargo test -p right-agent doctor`), then `cargo test --workspace` as the
  mandatory final check. The `ci_openshell_` test runs under the CI ignored
  filter, not locally by default.

## Files touched

- `crates/bot/src/cron.rs` — `consume_cron_stream` body + signature,
  `CronStreamOutcome` (drop `TimedOut`/deadline plumbing), `execute_job` gate,
  new unit tests.
- `crates/right-agent/src/doctor.rs` (+ `doctor_tests.rs`) — new running-cron
  check.
- `right-dashboard` — overview list of running cron runs (Vue view + internal
  API route/runtime-state wiring + tests).
- Existing repro test stays; flips RED→GREEN.

## Risks / notes

- **No-result tail still unbounded mid-flight.** Accepted: bounded by the 60 s
  shutdown-drain, now rare (common case fixed), and made visible by Part 3. If
  the monitor shows it recurring, revisit the deferred deadline.
- **Remote orphan invisibility.** A fully-detached in-sandbox process survives
  host killpg and host-side SIGHUP; it is invisible to the host. Part 3's
  signal (`running` async_runs row) is the proxy that surfaces it; a direct
  in-sandbox scan is out of scope.
- **`parse_stream_event` reuse** keeps cron and worker result-detection in
  sync; if the terminal-event shape changes, both update together.
