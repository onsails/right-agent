---
phase: 27-cron-runtime
plan: 01
subsystem: bot/cron
tags: [cron, scheduling, sqlite, migration, tokio]
dependency_graph:
  requires: [crates/rightclaw/src/memory, crates/bot/src/telegram/worker.rs]
  provides: [cron scheduling engine, V3 migration, cron_runs table]
  affects: [crates/bot/src/lib.rs, crates/rightclaw/src/memory/migrations.rs]
tech_stack:
  added: [cron = "0.16", serde-saphyr (bot), walkdir (bot)]
  patterns: [per-job tokio::spawn, lock file heartbeat, per-job DB connection, TDD RED/GREEN]
key_files:
  created:
    - crates/bot/src/cron.rs
    - crates/rightclaw/src/memory/sql/v3_cron_runs.sql
  modified:
    - crates/rightclaw/src/memory/migrations.rs
    - crates/rightclaw/src/memory/mod.rs
    - crates/bot/src/lib.rs
    - crates/bot/Cargo.toml
    - Cargo.toml
decisions:
  - "D-01: --agent <name> invocation style (not --system-prompt-file) — matches AGDEF-02"
  - "D-02: subprocess failures log tracing::error only, do not propagate"
  - "D-03: missed runs on restart are skipped (no catch-up logic)"
  - "D-04: cron_runs table in memory.db as V3 migration + log files at crons/logs/"
  - "Lock file format: JSON with heartbeat field, parsed per-job via is_lock_fresh"
  - "rusqlite::Connection is !Send — open_connection called per-job inside execute_job"
metrics:
  duration_seconds: 222
  completed_date: "2026-04-01"
  tasks_completed: 2
  files_changed: 7
---

# Phase 27 Plan 01: Cron Scheduling Engine Summary

Tokio-based cron runtime added to the bot process: `cron.rs` polls `crons/*.yaml` every 60s, spawns per-job loops that sleep until next schedule time, check lock files for deduplication, fire `claude -p --agent` subprocesses, and record structured run history in `memory.db` plus full stdout/stderr logs to `crons/logs/`.

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | V3 migration — cron_runs table | 258fb84 | v3_cron_runs.sql, migrations.rs, mod.rs |
| 2 | cron.rs scheduling engine | 8d9f4b6 | cron.rs, lib.rs, Cargo.toml x2 |

## What Was Built

**Task 1 — V3 migration:**
- `v3_cron_runs.sql`: DDL for `cron_runs` table with id (UUID PK), job_name, started_at, finished_at (nullable), exit_code (nullable), status (`running`/`success`/`failed`), log_path
- `migrations.rs`: Added `V3_SCHEMA` constant, extended `MIGRATIONS` vec to 3 elements
- `mod.rs`: Updated `user_version_is_2` → `user_version_is_3`; added `schema_has_cron_runs_table` and `cron_runs_insert_and_update` tests
- All 61 memory tests pass

**Task 2 — cron scheduling engine:**
- `crates/bot/src/cron.rs` (282 lines + 80 lines tests = ~362 total)
  - `CronSpec` struct: schedule, prompt, lock_ttl, max_turns — deserialized from YAML
  - `LockFile` struct: heartbeat field for JSON lock files
  - `CronError`: thiserror enum (BinaryNotFound, InvalidLockTtl, ScheduleParse, Io, Db)
  - `to_7field(expr)`: wraps 5-field user expression to 7-field cron crate format
  - `parse_lock_ttl(s)`: parses "30m"/"1h" strings to chrono::Duration
  - `is_lock_fresh(agent_dir, job_name, ttl)`: reads lock JSON, compares heartbeat to TTL
  - `load_specs(agent_dir)`: scans crons/*.yaml, returns HashMap<String, CronSpec>
  - `execute_job(...)`: full job execution: lock → DB insert → CC subprocess → log → DB update → lock delete
  - `update_run_record(conn, run_id, exit_code, status)`: UPDATE cron_runs on completion
  - `run_cron_task(agent_dir, agent_name)`: main reconciler, 60s interval
  - `reconcile_jobs(...)`: abort changed/removed handles, spawn new ones
  - `run_job_loop(...)`: per-job loop — sleep to next fire time, then execute_job
- `lib.rs`: Added `mod cron;`, spawned `cron::run_cron_task` before `telegram::run_telegram`
- `Cargo.toml` (workspace): Added `cron = "0.16"`
- `crates/bot/Cargo.toml`: Added serde-saphyr, walkdir, cron dependencies

**10 unit tests — all pass:**
- `test_to_7field_step`, `test_to_7field_specific`
- `test_parse_lock_ttl_minutes`, `test_parse_lock_ttl_hours`, `test_parse_lock_ttl_invalid`
- `test_is_lock_fresh_no_lock_file`, `test_is_lock_fresh_fresh_lock`, `test_is_lock_fresh_stale_lock`
- `test_load_specs_empty_dir`, `test_load_specs_valid_yaml`

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — all functionality is wired. The cron task starts but will find no jobs in `crons/` until the agent has YAML specs deployed (by cronsync skill, Phase 28).

## Requirements Addressed

- CRON-01: Bot process spawns cron task alongside Telegram dispatcher on startup
- CRON-02: Cron task reads crons/*.yaml every 60s
- CRON-03: Per-job tokio loops fire on schedule
- CRON-04: Lock file prevents duplicate runs; stale locks (beyond lock_ttl) cleared
- CRON-05: CC invoked via `claude -p --agent <name>` (D-01 takes precedence over CRON-05 wording)
- CRON-06: Re-reading changed cron specs aborts old job handles and spawns fresh ones

## Self-Check: PASSED
