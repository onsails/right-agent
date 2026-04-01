# Phase 28: Cronsync SKILL Rewrite — Context

**Gathered:** 2026-04-01
**Status:** Ready for planning

<domain>
## Phase Boundary

Rewrite `skills/cronsync/SKILL.md` from a 295-line reconciler (CHECK/RECONCILE/CronCreate/CronDelete/CronList)
down to a focused file-management skill. The Rust runtime (Phase 27, `crates/bot/src/cron.rs`) now owns
all execution: scheduling, lock files, log capture, DB tracking.

The skill's only job after this phase: help agents create, edit, and delete YAML spec files in `crons/`.

</domain>

<decisions>
## Implementation Decisions

### D-01: Skill activation — reactive only
`/rightcron` activates ONLY when the user explicitly asks to manage cron specs (create/edit/delete a job).
No startup behavior. No bootstrap. No health check on session start.

Frontmatter `description` must be updated to reflect pure file management — remove "On EVERY session
startup, schedules a periodic reconciler and recovers persisted jobs."

### D-02: MCP observability section — full section with examples
Add a dedicated **"Checking Run History"** section documenting `cron_list_runs` and `cron_show_run`
from the `rightclaw` MCP server.

Required content:
- When to use each tool (user asks about run status, debugging a failed job)
- `cron_list_runs(job_name?: str, limit?: int)` — parameters explained, returns array sorted by `started_at DESC`
- `cron_show_run(run_id: str)` — single run metadata; returns graceful "not found" for unknown IDs
- `log_path` field: agent reads the log file directly via bash (no MCP tool for log content)
- Example: user asks "why did morning-briefing fail?" → `cron_list_runs("morning-briefing", 5)` → pick a `run_id` → `cron_show_run(run_id)` → read `log_path`

### D-03: Constraints section — minimal (UTC + 60s delay only)
Drop all CC-specific constraints (50-task limit, 3-day auto-expiry). Keep exactly two notes:
1. **UTC schedules**: cron expressions are evaluated in UTC by the Rust runtime. Write specs accordingly
   (e.g., `0 9 * * 1-5` fires at 09:00 UTC, not local time). Old skill documented LOCAL timezone — this is
   a behavior change.
2. **60s polling**: the runtime re-reads `crons/*.yaml` every 60 seconds. After writing/editing/deleting
   a spec file, changes take effect within ~1 minute.

### D-04: Remove all CC tool references
Remove every reference to: `CronCreate`, `CronDelete`, `CronList`, Agent tool guard, reconciliation
algorithm, state.json, lock guard wrapper construction, BOOT-01/BOOT-02.

Lock files (`crons/.locks/`) and log files (`crons/logs/`) are Rust-managed runtime artifacts — not
documented in the skill (agent shouldn't interact with them directly, except reading log_path from MCP).

### D-05: Audit with /create-agent-skill
The plan MUST include a step to run the `/create-agent-skill` skill (compound-engineering skill) against
the rewritten SKILL.md to audit it and fix any issues the skill finds. This ensures SKILL.md format
compliance and quality before the phase is considered done.

### Claude's Discretion
- Exact wording and section order in the new SKILL.md
- Whether to keep the "Important Rules" section (simplified) or drop it entirely
- YAML spec format table: keep as-is (fields are correct — Rust parses all 4 fields)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

- `skills/cronsync/SKILL.md` — current skill (full rewrite target; read to understand what to remove)
- `crates/bot/src/cron.rs` — Rust cron runtime (what fields CronSpec parses, how lock/log/DB work)
- `crates/rightclaw-cli/src/memory_server.rs` — MCP server with `cron_list_runs`/`cron_show_run` tool implementations and their parameter structs
- `.planning/phases/27-cron-runtime/27-CONTEXT.md` — Phase 27 decisions D-01..D-06 (execution model, MCP tools, deferred skill docs)
- `.planning/REQUIREMENTS.md` — SKILL-01, SKILL-02, SKILL-03

</canonical_refs>

<code_context>
## Existing Code Insights

### Current skill state
- `skills/cronsync/SKILL.md`: 295 lines, version 0.2.0, name `rightcron`
- Contains: bootstrap, reconciliation algorithm (Steps 1-6), CRITICAL guard, lock guard wrapper
- YAML spec format table is correct and must be kept (4 fields match Rust CronSpec struct)
- "When to Activate" section needs narrowing — remove startup trigger

### Runtime reality (Phase 27)
- `CronSpec { schedule, prompt, lock_ttl: Option<String>, max_turns: Option<u32> }` — all 4 YAML fields parsed
- Lock files: Rust writes `{"heartbeat": "..."}` at `crons/.locks/<name>.json`, reads/deletes them
- Log files: Rust writes to `crons/logs/<job_name>-<run_id>.txt` (stdout + stderr)
- `cron_runs` table: `id, job_name, started_at, finished_at, exit_code, status, log_path`
- MCP server name: `rightclaw` (renamed in Phase 27 Plan 02)

### MCP tool signatures
- `cron_list_runs(job_name?: str, limit?: int)` → array of run rows, sorted started_at DESC, default limit 20
- `cron_show_run(run_id: str)` → single run row or "not found" message (not an MCP error)

</code_context>

<deferred>
## Deferred Ideas

None raised during discussion.

</deferred>

---

*Phase: 28-cronsync-skill-rewrite*
*Context gathered: 2026-04-01*
