# Cron Log Sandbox Streaming

## Goal

Move cron job NDJSON logs from the host into the sandbox so agents can read them directly with `Read`, eliminating the need for MCP log-reading tools.

## Current State

- Bot runs `claude -p` via SSH in sandbox, reads stdout on host
- NDJSON stream log written to host: `~/.rightclaw/logs/streams/{job_name}-{run_id}.ndjson`
- Text summary log written to host: `agents/<name>/crons/logs/{job_name}-{run_id}.txt`
- `log_path` in `cron_runs` DB stores host path
- Agents cannot access either file — they're outside the sandbox

## Design

### 1. Tee stdout into sandbox

Modify the shell command that runs CC inside the sandbox to tee stdout to a file:

```
mkdir -p /sandbox/crons/logs && <prompt_assembly> | claude -p ... | tee /sandbox/crons/logs/{job_name}-{run_id}.ndjson
```

Bot still reads stdout through the SSH pipe (tee passes it through). NDJSON lands inside the sandbox in real time.

For `--no-sandbox` mode, the path is `{agent_dir}/crons/logs/{job_name}-{run_id}.ndjson`.

### 2. Remove host-side log files

Bot no longer writes:
- `~/.rightclaw/logs/streams/{job_name}-{run_id}.ndjson` (host NDJSON)
- `agents/<name>/crons/logs/{job_name}-{run_id}.txt` (host text summary)

All log writing code on the host side is removed from `execute_job`.

### 3. `log_path` in DB

`log_path` column in `cron_runs` stores the sandbox-relative path: `/sandbox/crons/logs/{job_name}-{run_id}.ndjson`.

For no-sandbox: `{agent_dir}/crons/logs/{job_name}-{run_id}.ndjson`.

No schema migration needed — same column, different value.

### 4. Log retention

After each cron run completes, clean up old log files for that job:
- List files matching `{job_name}-*.ndjson` in the logs directory
- Sort by modification time
- Delete all but the 10 most recent

Deletion happens inside the sandbox (via the same SSH session) for sandbox mode, or directly on the filesystem for no-sandbox mode.

### 5. SKILL.md update

Add "Watching a running job" section teaching agents to:
1. Call `cron_list_runs(job_name, limit=1)` to find a running job
2. `Read` the tail of `log_path` to see current activity (use `offset` parameter to read only the end)
3. Re-read periodically to follow progress

Replace existing `cat <log_path>` instructions with `Read` tool usage. Emphasize reading the tail for large logs.

### 6. ARCHITECTURE.md update

Clarify that sandboxes are persistent and live as long as the agent lives (not limited lifetime).

Update the cron logging section to reflect that logs live inside the sandbox.

## Non-goals

- No new MCP tools
- No new DB columns or migrations
- No live-streaming / push notifications
- No log aggregation across agents

## Files to modify

| File | Change |
|------|--------|
| `crates/bot/src/cron.rs` | Add tee to shell command, remove host log writing, add retention cleanup, update `log_path` value |
| `skills/rightcron/SKILL.md` | Add "Watching a running job" section, replace `cat` with `Read`, emphasize tail reading |
| `ARCHITECTURE.md` | Clarify sandbox persistence, update log paths |
