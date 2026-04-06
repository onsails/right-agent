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

## Milestone: v3.1 — Sandbox Fix & Verification

**Shipped:** 2026-04-03
**Phases:** 3 | **Plans:** 3 | **Sessions:** 2

### What Was Built
- `sandbox.ripgrep.command` injected into per-agent settings.json via `which::which("rg")` at `rightclaw up` time; `USE_BUILTIN_RIPGREP` polarity corrected; `failIfUnavailable:true` added unconditionally
- `rightclaw doctor` extended with `check_rg_in_path()` (Linux, Warn) and `check_ripgrep_in_settings()` (cross-platform, per-agent Warn); tests extracted to `doctor_tests.rs` per 900-line rule
- `tests/e2e/verify-sandbox.sh` — 4-stage bash pipeline proving CC sandbox engagement via exit-code strategy under `failIfUnavailable:true`; live-confirmed 2026-04-03

### What Worked
- **Fail-fast over silent-fail design:** Adding `failIfUnavailable:true` unconditionally (even with `--no-sandbox`) means broken sandbox configs fail loudly at CC launch — much easier to diagnose than the previous silent degradation
- **Exit-code proof for E2E:** Rejecting stderr grep in favour of exit-code-under-failIfUnavailable was the right call. Stderr messages are brittle across CC versions; exit code is stable. Caught by audit: VER-01 comment overclaims, but proof is solid.
- **TDD for doctor checks:** All 41 doctor tests pass. The test-first approach for check_ripgrep_in_settings caught edge cases (absent key, non-executable path, invalid JSON) that were easy to miss.
- **Live run during execute-phase:** Running verify-sandbox.sh immediately after execution rather than deferring to a separate verification phase caught the `claude` → `claude-bun` binary issue right away.

### What Was Inefficient
- **`rightclaw up` for settings regeneration requires a TTY:** Had to use `--detach` workaround to regenerate settings.json during E2E testing. A `rightclaw regen-settings` or similar standalone command would be cleaner.
- **Phase 31 plan didn't account for `claude-bun` binary:** The plan hardcoded `claude` in Stage 4. The fix was trivial (1 line) but the gap should have been caught in planning since `claude-bun` is documented in project memory.
- **Merge conflict from worktree execution:** The parallel executor worktree had stale data for `completed_phases` and `total_plans` in STATE.md, requiring manual conflict resolution. The gsd-tools `begin-phase` call (which runs before the worktree forks) and the worktree's completion should reconcile more cleanly.

### Patterns Established
- **E2E scripts live in `tests/e2e/`**, `last-run.log` excluded via `.gitignore` — first entry in this directory
- **CC binary resolution:** Always use `which("claude").or_else(|_| which("claude-bun"))` pattern — mirrors `worker.rs` and handles nix vs system installs
- **Sandbox proof via failIfUnavailable:** Exit 0 under `failIfUnavailable:true` is the canonical way to prove sandbox engagement — document this as the pattern for future verification scripts

### Key Lessons
1. **Live run immediately:** Don't defer E2E testing to a separate verification phase. Run the script right after execution — catches environment issues (missing binaries, missing settings.json fields) while context is fresh.
2. **Plan environment assumptions explicitly:** `claude-bun` vs `claude` is documented in memory but wasn't in the plan. Plan steps that invoke external binaries should explicitly note fallback binary names.
3. **failIfUnavailable is your friend:** Make sandbox failures fatal early. Silent degradation is the worst failure mode for security-relevant features — it masks real problems and creates false confidence.

### Cost Observations
- Small, focused milestone: 3 phases, 3 plans, 2 days
- Most expensive part was the E2E script execution (CC API calls cost ~$0.06 per smoke test run)
- Doctor check extraction to `doctor_tests.rs` was overhead but necessary for 900-line compliance

---

## Cross-Milestone Trends

| Milestone | Phases | Plans | What Shipped | Outcome |
|-----------|--------|-------|-------------|---------|
| v3.1 Sandbox Fix & Verification | 3 | 3 | rg injection, failIfUnavailable, doctor, E2E script | ✓ Full delivery, live-confirmed |
| v2.5 RightCron Reliability | 1+1cancelled | 2 | Inline bootstrap + CHECK/RECONCILE redesign | ✓ Code shipped, E2E deferred |
| v2.4 Sandbox Telegram Fix | 1 | 1 | CC channels bug diagnosis | Deferred fix to CC |
| v2.3 Memory System | 4 | 9 | SQLite memory, MCP server, CLI inspection | ✓ Full delivery |
| v2.2 Skills Registry | 5 | 5 | rightskills, env injection, policy gate | ✓ Full delivery |
