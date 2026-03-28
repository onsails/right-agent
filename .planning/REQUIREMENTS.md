# Requirements: v2.5 RightCron Reliability

## Milestone Goal

Make rightcron's cron reconciler actually work — fix the bootstrap so it can create the reconciler job, and redesign the skill so reconciliation is deterministic regardless of context.

## Active Requirements

### Bootstrap Fix

- [ ] **BOOT-01**: `rightclaw up` results in a `*/5 * * * *` reconciler cron job existing in the agent session (CronList confirms it)
- [ ] **BOOT-02**: `startup_prompt` does not delegate to a background Agent tool — rightcron runs inline in the main thread

### Reconciler Redesign

- [ ] **RECON-01**: rightcron skill separates reconciler into CHECK (read-only, outputs structured diff — no CronCreate/CronDelete calls) and RECONCILE (direct CronCreate/CronDelete in main thread based on diff)
- [ ] **RECON-02**: After cron fires, jobs defined in `crons/*.yaml` are created/updated/deleted correctly without any Agent tool delegation

### Verification

- [ ] **VER-01**: Manual end-to-end test — create a `crons/*.yaml` spec, wait for reconciler to fire (or trigger manually), confirm job is scheduled via CronList

## Future Requirements

- Automated test for cron reconciler cycle (deferred — requires live CC session)
- Multi-agent cron isolation (deferred — all agents share session-scoped crons today)

## Out of Scope

- Fixing CC iv6/M6 channels bug (waiting for CC upstream)
- Changing cron reconciler interval (stays `*/5 * * * *`)
- rightcron conversational job creation/removal (no changes to non-reconciler flows)

## Traceability

| Requirement | Phase | Plan |
|-------------|-------|------|
| BOOT-01 | Phase 21 | TBD |
| BOOT-02 | Phase 21 | TBD |
| RECON-01 | Phase 21 | TBD |
| RECON-02 | Phase 21 | TBD |
| VER-01 | Phase 22 | TBD |
