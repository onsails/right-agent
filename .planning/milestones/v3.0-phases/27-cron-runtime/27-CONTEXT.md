# Phase 27: Cron Runtime — Context

## Domain

Tokio task inside `rightclaw bot` process that polls `crons/*.yaml` every 60s and fires
`claude -p` subprocesses on schedule, with lock-file-based deduplication.

Runs alongside the Telegram dispatcher in `crates/bot/src/`. New module: `crates/bot/src/cron.rs`.

## Canonical refs

- `crates/bot/src/lib.rs` — bot entry point, where cron task is spawned
- `crates/bot/src/telegram/worker.rs` — `invoke_cc` pattern to reuse for cron subprocess invocation
- `crates/rightclaw/src/agent/types.rs` — `AgentConfig` (no cron fields; YAML specs handle config)
- `crates/rightclaw/src/memory/mod.rs` — `open_connection`, migration infrastructure
- `crates/rightclaw/src/memory/migrations.rs` — add `cron_runs` table here (V3 migration)
- `crates/rightclaw-cli/src/memory_server.rs` — MCP server to extend with cron tools
- `skills/cronsync/SKILL.md` — lock file format, YAML spec fields, `lock_ttl` default (30m)
- `.planning/REQUIREMENTS.md` — CRON-01 through CRON-06

## Decisions

### D-01: CC invocation style

Cron jobs use `--agent <name>` — same as bot dispatch (AGDEF-02). Full agent def inherited:
IDENTITY.md + SOUL.md + tool whitelist from sandbox config.

**Not used**: `--output-format json`, `--json-schema` (cron jobs don't return structured replies —
they act autonomously and communicate via `reply` MCP tool).

Command shape:
```
claude -p --agent <name> -- "<job prompt wrapped with lock guard>"
```
Same `HOME` and `cwd` as bot dispatch (`agent_dir`).

### D-02: Subprocess failure routing

Tracing logs only. `tracing::error!(job = %name, exit_code, "cron job subprocess failed")`.

Rationale: cron jobs communicate with users via the `reply` MCP tool themselves. Subprocess
failure is an infra-level event (CC crash, binary missing, etc.) — belongs in logs, not Telegram.

### D-03: Missed runs on restart — Skip

If the bot was down during a scheduled window, the run is skipped. Next execution is at the
next scheduled time after startup. No catch-up logic.

### D-04: Cron run history (scope folded into Phase 27)

**Storage**: Two layers:
1. `cron_runs` table in `memory.db` — structured metadata only
2. Log files at `agent_dir/crons/logs/<job_name>-<run_id>.txt` — full stdout+stderr

**Table schema** (`cron_runs`, V3 migration):
```sql
CREATE TABLE cron_runs (
    id          TEXT PRIMARY KEY,        -- UUID
    job_name    TEXT NOT NULL,
    started_at  TEXT NOT NULL,           -- ISO8601 UTC
    finished_at TEXT,                    -- NULL while running
    exit_code   INTEGER,                 -- NULL while running
    status      TEXT NOT NULL,           -- 'running' | 'success' | 'failed'
    log_path    TEXT NOT NULL            -- absolute path to log file
);
```

**Write pattern**: Insert `status='running'` at job start; UPDATE to `success`/`failed` on
completion. Allows agent to detect stuck jobs (status='running', old started_at).

**Log files**: Full subprocess stdout+stderr captured to `crons/logs/<job_name>-<run_id>.txt`.
Agent reads log files directly via bash/file tools — MCP only surfaces the path.

### D-05: MCP server — rename + extend

- **Server name**: `rightclaw` (was "memory"). Update `get_info()` server name in `memory_server.rs`.
- **New tools** added to existing `MemoryServer` (same struct, same file):
  - `cron_list_runs(job_name?: str, limit?: int)` — returns recent runs sorted by `started_at` DESC.
    Default limit: 20. Returns array of: `{id, job_name, started_at, finished_at, status, exit_code, log_path}`.
  - `cron_show_run(run_id: str)` — returns full metadata for one run (same fields). Agent reads
    the log file separately via bash using `log_path`.
- No `cron_read_log` MCP tool — agent reads log files directly.

### D-06: Skill updates deferred to Phase 28

Phase 28 ("Cronsync SKILL Rewrite") will add `cron_list_runs`/`cron_show_run` usage instructions
to the skill. Phase 27 delivers the runtime and MCP tools only.

## Scope additions vs original requirements

Phase 27 now includes (beyond CRON-01..06):
- `cron_runs` table migration (V3) in `memory.db`
- Log file capture to `crons/logs/`
- MCP rename + `cron_list_runs` + `cron_show_run` tools

## Out of scope / deferred

- File-watch hot-reload (notify-debouncer-full) — REQUIREMENTS.md marks as v3.1
- `catch_up: true` field in YAML spec — can add later
- Skill update for cron run querying — Phase 28
