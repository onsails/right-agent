# Phase 21: Fix & Verification - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-03-28
**Phase:** 21-fix-verification
**Areas discussed:** Fix mechanism, Verification approach, Scope cleanup

---

## Fix Mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Persistent background agent (A1/A2) | rightcron loops or separate heartbeat | |
| idle_prompt hook (D) | hook forces M6() on idle | |
| Fix sub-agent model → CronCreate works | Ensure Sonnet for rightcron sub-agent | ✓ |

**User's choice:** Fix the model so the background sub-agent can access CronCreate.
**Notes:** Background sub-agent defaults to Haiku → tool_reference disabled → CronCreate search
fails → reconciler job never created → SubagentStop → freeze. Fix: ensure Sonnet for sub-agent.
User noted research should investigate CC model selection — hooks may run on Haiku and that
might be the source of the "tool search disabled for haiku" log line.

---

## Verification Approach

| Option | Description | Selected |
|--------|-------------|----------|
| 5 minutes is fine | Wait one cron fire cycle | ✓ |
| Use 1-minute cron interval | Shorter wait for testing | |

**User's choice:** 5 minutes acceptable for verification.
**Notes:** Confirmation Test A from DIAGNOSIS.md is the pass/fail gate.

---

## Scope Cleanup

| Option | Description | Selected |
|--------|-------------|----------|
| Update requirements text in this phase | Fix DIAG-02/DIAG-03 stale text | |
| Defer to milestone wrap-up | Accept stale text for now | ✓ |

**User's choice:** Defer to milestone wrap-up.
