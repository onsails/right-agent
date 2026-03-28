# Project Retrospective

*A living document updated after each milestone. Lessons feed forward into future planning.*

---

## Milestone: v2.4 — Sandbox Telegram Fix

**Shipped:** 2026-03-28
**Phases:** 1 | **Plans:** 1 | **Sessions:** 1

### What Was Built
- Root cause diagnosis of CC Telegram channel freeze after SubagentStop
- DIAGNOSIS.md with full evidence trail, process topology proof, cli.js source analysis
- SEED-011 documenting the fix for when CC ships the upstream fix

### What Worked
- Starting with log analysis before jumping to code — revealed the real problem quickly
- Hypothesis-elimination approach: Hypothesis B (socat) was cleanly eliminated via live process topology before wasting time on it
- Deciding to stop and wait for CC rather than shipping a fragile workaround

### What Was Inefficient
- Initial framing ("sandbox blocks Telegram") was wrong — wasted early discussion time on sandbox-specific angles
- DIAG-02/DIAG-03 requirements were written before diagnosis; had to accept stale text rather than reworking them mid-stream
- Went through discuss-phase → plan-phase → then dropped the work; could have diagnosed first before planning Phase 21

### Patterns Established
- When a bug "works without X", first verify the test was actually equivalent before assuming X is the cause
- CC background agents (Agent tool) don't have access to CronCreate — it's a main-thread-only built-in
- Debug log silence after idle_prompt is normal CC behavior, not evidence of a problem

### Key Lessons
1. The "works without --no-sandbox" observation was confounded — the test sessions didn't run rightcron at all. Test equivalence before blaming a variable.
2. CC's `iv6` callback only aborts running operations; it never drains the `hz` queue from idle state. Any feature depending on "waking up" CC from idle (channels, hooks) has this latent bug.
3. Research before planning Phase 21 saved significant wasted execution — the haiku log line looked like the fix but was a red herring. Research resolved it cheaply.

### Cost Observations
- Single-day milestone: investigate-heavy, 22 planning commits, 0 code commits
- Diagnosis milestones have lower cost/value ratio than feature milestones — but prevent wasted implementation effort

---

## Cross-Milestone Trends

| Milestone | Phases | Plans | What Shipped | Outcome |
|-----------|--------|-------|-------------|---------|
| v2.4 Sandbox Telegram Fix | 1 | 1 | CC channels bug diagnosis | Deferred fix to CC |
| v2.3 Memory System | 4 | 9 | SQLite memory, MCP server, CLI inspection | ✓ Full delivery |
| v2.2 Skills Registry | 5 | 5 | rightskills, env injection, policy gate | ✓ Full delivery |
