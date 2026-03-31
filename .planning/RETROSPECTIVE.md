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

## Milestone: v2.5 — RightCron Reliability

**Shipped:** 2026-03-31
**Phases:** 1 complete, 1 cancelled | **Plans:** 2 | **Sessions:** 1

### What Was Built
- TDD fix: startup_prompt regression tests + inline bootstrap (CronCreate now accessible on main thread)
- cronsync SKILL.md restructured: CRITICAL guard at algorithm entry + CHECK/RECONCILE phase split
- Phase 22 (E2E verification) cancelled — user chose new milestone approach instead

### What Worked
- TDD approach for startup_prompt fix: RED test first confirmed the bug, GREEN fix confirmed the repair — 2 tasks in ~8 min
- Structural prevention over documentation: the CHECK/RECONCILE split makes delegation physically impossible at algorithm level, not just forbidden by text
- CRITICAL guard blockquote placed before the first step — catches the constraint before any execution path is read

### What Was Inefficient
- Phase 22 (E2E verification) planned but never started — skipped in favour of new approach. Planning it was wasted overhead.
- Milestones with a single verification phase as final step are fragile: user can always decide to pivot before manual testing.

### Patterns Established
- When fixing "LLM ignores instructions" bugs: structural separation (new section headers, distinct commands) beats inline warnings
- CC built-in tools (CronCreate/CronDelete/CronList) are main-thread-only — background Agent tool delegates can't access them
- "CRITICAL: NEVER use the Agent tool" blockquote at section entry is the established guard pattern for this class of bug

### Key Lessons
1. Manual E2E verification phases are a risk — user can always decide the actual verification is a new milestone worth doing differently. Don't let them block shipping the code fix.
2. Structural fixes (CHECK/RECONCILE split) are more reliable than LLM prompt engineering alone. The skill redesign forces the right execution order architecturally.
3. The startup_prompt Agent tool delegation bug was a design error in the original CronSync implementation — the "run in background" pattern is incompatible with main-thread-only tools.

### Cost Observations
- Small milestone: 2 tasks, 2 commits (+53/-8 lines of code), fast execution
- TDD overhead paid off immediately: tests caught the regression before production and documented the expected invariant

---

## Cross-Milestone Trends

| Milestone | Phases | Plans | What Shipped | Outcome |
|-----------|--------|-------|-------------|---------|
| v2.5 RightCron Reliability | 1+1cancelled | 2 | Inline bootstrap + CHECK/RECONCILE redesign | ✓ Code shipped, E2E deferred |
| v2.4 Sandbox Telegram Fix | 1 | 1 | CC channels bug diagnosis | Deferred fix to CC |
| v2.3 Memory System | 4 | 9 | SQLite memory, MCP server, CLI inspection | ✓ Full delivery |
| v2.2 Skills Registry | 5 | 5 | rightskills, env injection, policy gate | ✓ Full delivery |
