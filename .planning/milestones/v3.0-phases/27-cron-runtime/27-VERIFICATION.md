---
phase: 27-cron-runtime
verified: 2026-04-01T19:30:00Z
status: passed
score: 11/11 must-haves verified
re_verification: false
---

# Phase 27: Cron Runtime Verification Report

**Phase Goal:** Cron jobs defined in crons/*.yaml are scheduled and executed by a Rust tokio task inside the bot process
**Verified:** 2026-04-01T19:30:00Z
**Status:** passed
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Bot process spawns a cron task alongside the Telegram dispatcher on startup | VERIFIED | `lib.rs:103-108` — `tokio::spawn(async move { cron::run_cron_task(cron_agent_dir, cron_agent_name).await })` placed before `telegram::run_telegram(...)` |
| 2 | Cron task reads crons/*.yaml every 60s and launches per-job tokio loops | VERIFIED | `cron.rs:278-290` — `run_cron_task` uses 60s `tokio::time::interval`, calls `reconcile_jobs` on startup and each tick |
| 3 | Lock file prevents duplicate runs; stale locks (beyond lock_ttl) are cleared | VERIFIED | `cron.rs:129-147` — `is_lock_fresh` checked before execution; lock deleted at job completion (`cron.rs:255`) |
| 4 | Completed job run is recorded in cron_runs table with exit_code and log_path | VERIFIED | `cron.rs:171-178` INSERT at start (status='running'); `cron.rs:252` `update_run_record` on completion; V3 migration confirmed |
| 5 | Log file at crons/logs/<job_name>-<run_id>.txt contains full subprocess output | VERIFIED | `cron.rs:153-159,228-236` — log dir created, stdout+stderr concatenated and written to `{job_name}-{run_id}.txt` |
| 6 | Re-reading changed cron specs aborts old job handles and spawns fresh ones | VERIFIED | `cron.rs:299-332` — `reconcile_jobs` aborts changed/removed handles via `handle.abort()`, spawns replacements |
| 7 | MCP server advertises name 'rightclaw' | VERIFIED | `memory_server.rs:246-249` — `Implementation::new("rightclaw", env!("CARGO_PKG_VERSION"))` |
| 8 | Agent can call cron_list_runs() to filter and view job run history | VERIFIED | `memory_server.rs:161-198` — full implementation with `job_name` filter and `limit`, queries `cron_runs` table |
| 9 | Agent can call cron_show_run(run_id='...') to get metadata for one run | VERIFIED | `memory_server.rs:200-239` — queries by id, returns graceful "not found" for missing IDs |
| 10 | Both tools return log_path | VERIFIED | `cron_run_to_json` helper at `memory_server.rs:268-286` includes `log_path` field |
| 11 | .mcp.json key 'rightmemory' unchanged; only ServerInfo display name changes | VERIFIED | No changes to .mcp.json generation; only `get_info()` updated |

**Score:** 11/11 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/bot/src/cron.rs` | CronSpec, run_cron_task, execute_job, lock check, DB insert/update | VERIFIED | 471 lines (282 impl + 80 tests + comments); all required functions present |
| `crates/rightclaw/src/memory/sql/v3_cron_runs.sql` | V3 migration DDL for cron_runs table | VERIFIED | 11 lines; `CREATE TABLE IF NOT EXISTS cron_runs` with all required columns |
| `crates/rightclaw/src/memory/migrations.rs` | Updated MIGRATIONS with V3_SCHEMA | VERIFIED | 10 lines; `V3_SCHEMA` constant and 3-element `MIGRATIONS` vec |
| `crates/rightclaw-cli/src/memory_server.rs` | Renamed MCP server + cron_list_runs + cron_show_run tools | VERIFIED | 492 lines; `cron_list_runs`, `cron_show_run`, `Implementation::new("rightclaw", ...)` all present |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `crates/bot/src/lib.rs` | `crates/bot/src/cron.rs` | `tokio::spawn(cron::run_cron_task(...))` | WIRED | `lib.rs:1` has `pub mod cron;`; `lib.rs:106-108` spawns task |
| `crates/bot/src/cron.rs` | `crates/rightclaw/src/memory` | `open_connection(&agent_dir)` per execute_job | WIRED | `cron.rs:163` — `rightclaw::memory::open_connection(agent_dir)` inside `execute_job` |
| `crates/bot/src/cron.rs` | `cron_runs table` | INSERT at start, UPDATE on completion | WIRED | `cron.rs:171-178` INSERT; `cron.rs:267-272` UPDATE; `cron.rs:252` calls `update_run_record` |
| `MemoryServer::get_info()` | `ServerInfo with_server_info` | `Implementation::new("rightclaw", ...)` | WIRED | `memory_server.rs:246-249` |
| `cron_list_runs / cron_show_run` | `cron_runs table` | conn lock + rusqlite query | WIRED | `memory_server.rs:173-177` and `memory_server.rs:210-211` query `cron_runs` |

---

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|-------------------|--------|
| `cron.rs execute_job` | subprocess stdout/stderr | `child.wait_with_output().await` | Yes — actual subprocess output | FLOWING |
| `cron.rs execute_job` | DB row | `open_connection` + `conn.execute(INSERT/UPDATE)` | Yes — real rusqlite write | FLOWING |
| `memory_server.rs cron_list_runs` | rows Vec | `stmt.query_map(...)` on cron_runs | Yes — real rusqlite query | FLOWING |
| `memory_server.rs cron_show_run` | result Value | `conn.query_row(...)` on cron_runs | Yes — real rusqlite query | FLOWING |

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `parse_lock_ttl("30m")` returns Duration::minutes(30) | `cargo test -p rightclaw-bot test_parse_lock_ttl_minutes` | 1 test passed | PASS |
| `to_7field("*/5 * * * *")` returns 7-field | `cargo test -p rightclaw-bot test_to_7field_step` | 1 test passed | PASS |
| `is_lock_fresh` returns false for stale lock | `cargo test -p rightclaw-bot test_is_lock_fresh_stale_lock` | 1 test passed | PASS |
| `load_specs` reads valid YAML → HashMap | `cargo test -p rightclaw-bot test_load_specs_valid_yaml` | 1 test passed | PASS |
| cron_runs user_version is 3 | `cargo test -p rightclaw memory::tests::user_version_is_3` | 1 test passed | PASS |
| cron_list_runs returns empty array | `cargo test -p rightclaw-cli test_cron_list_runs_empty` | 1 test passed | PASS |
| cron_show_run returns "not found" | `cargo test -p rightclaw-cli test_cron_show_run_not_found` | 1 test passed | PASS |
| workspace build clean | `cargo build --workspace` | 0 errors | PASS |

All 10 cron unit tests pass (cron.rs), all 7 memory_server tests pass, all 61 rightclaw memory tests pass.

**Pre-existing failure:** `test_status_no_running_instance` in `rightclaw-cli` integration tests fails with HTTP error — documented in MEMORY.md as a pre-existing issue unrelated to phase 27.

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| CRON-01 | 27-01-PLAN | Cron runtime runs as tokio task inside bot process | SATISFIED | `lib.rs:103-108` spawns `cron::run_cron_task` before `telegram::run_telegram` |
| CRON-02 | 27-01-PLAN | Runtime reads crons/*.yaml on startup and every 60 seconds | SATISFIED | `cron.rs:280,284,288` — 60s interval, reconcile on startup and each tick |
| CRON-03 | 27-01-PLAN | Schedule parsed via cron 0.16 → chrono::DateTime for next-run | SATISFIED | `cron.rs:342-369` — `cron::Schedule::from_str` + `schedule.after(&now).next()` |
| CRON-04 | 27-01-PLAN | Lock file check before executing — skips if lock fresh | SATISFIED | `cron.rs:129-147` check + `cron.rs:255` delete on completion |
| CRON-05 | 27-01-PLAN | CC invoked as subprocess with same HOME/$cwd | SATISFIED (with supersession) | `cron.rs:192-204` — `claude -p --agent <name>` per D-01 (overrides `--system-prompt-file` wording in REQUIREMENTS.md, consistent with AGDEF-02 decision) |
| CRON-06 | 27-01-PLAN | Reconciler is idempotent — changed specs abort old handles | SATISFIED | `cron.rs:299-332` — diff-based abort + respawn in `reconcile_jobs` |

**Note on CRON-05:** REQUIREMENTS.md uses `--system-prompt-file` wording from before AGDEF-02. D-01 in 27-CONTEXT.md explicitly overrides this to use `--agent <name>`, consistent with the broader project direction (AGDEF-02). The PLAN itself notes this supersession. The implementation is correct.

**Orphaned requirements check:** REQUIREMENTS.md traceability table maps CRON-01 through CRON-06 to Phase 27. All six are covered by 27-01-PLAN. No orphaned requirements.

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None | — | — | — | No stubs, placeholders, or hollow implementations detected |

Checked: `cron.rs`, `memory_server.rs`, `migrations.rs`, `v3_cron_runs.sql`, `lib.rs`.
No TODO/FIXME/placeholder markers. No empty handlers. No static returns masking missing DB queries.

---

### Human Verification Required

#### 1. End-to-end cron execution in live agent

**Test:** Deploy an agent with a `crons/test-job.yaml` containing a short schedule (every minute). Run `rightclaw up` and `rightclaw bot --agent <name>`. Wait 2 minutes.
**Expected:** `crons/logs/test-job-<uuid>.txt` appears with subprocess output; `memory.db` contains a `cron_runs` row with `status='success'`.
**Why human:** Requires live CC binary, live agent directory, and wall-clock time — cannot verify programmatically.

#### 2. Lock file deduplication under slow job

**Test:** Create a cron job with a prompt that takes >1 minute and schedule it `* * * * *` (every minute). Run for 3 minutes.
**Expected:** Only one execution runs at a time; the second scheduled tick finds the lock fresh and skips.
**Why human:** Requires real subprocess execution with controlled timing.

#### 3. MCP tool visibility to agent via .mcp.json

**Test:** Start the `rightclaw mcp` server for an agent. Call `cron_list_runs` via MCP client.
**Expected:** Tool returns `[]` (empty) with no error; server display name shows "rightclaw".
**Why human:** Requires live MCP server and client handshake to verify `Implementation::new("rightclaw", ...)` is transmitted correctly in the `InitializeResult`.

---

### Gaps Summary

No gaps. All must-haves verified, all requirement IDs satisfied, workspace builds clean.

---

_Verified: 2026-04-01T19:30:00Z_
_Verifier: Claude (gsd-verifier)_
