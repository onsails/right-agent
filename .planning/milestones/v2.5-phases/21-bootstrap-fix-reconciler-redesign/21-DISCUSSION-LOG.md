# Phase 21: Bootstrap Fix + Reconciler Redesign - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-03-28
**Phase:** 21-bootstrap-fix-reconciler-redesign
**Areas discussed:** startup_prompt wording, CHECK→RECONCILE coupling, SKILL.md guard rule

---

## startup_prompt wording

| Option | Description | Selected |
|--------|-------------|----------|
| Silent bootstrap, no note | Clean minimal prompt | ✓ |
| Keep corrected threading note | Add "Run this inline — do not use the Agent tool" | |
| You decide | Claude picks | |

**User's choice:** Silent bootstrap, no threading note.
**Notes:** Remove entire "IMPORTANT: run this as a background agent..." sentence. Keep "do this silently without messaging the user."

---

## CHECK→RECONCILE coupling

| Option | Description | Selected |
|--------|-------------|----------|
| Internal two-step in same turn | Cron fires Run /rightcron reconcile, skill handles check then act internally | ✓ |
| Separate cron fires check command | Cron fires Run /rightcron check, explicit diff message, main thread acts | |

**User's choice:** Internal two-step in same turn.
**Notes:** Keep `Run /rightcron reconcile` as the cron prompt. Split is internal to the SKILL.md algorithm.

---

## SKILL.md guard rule

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — add CRITICAL constraint | Prominent rule: NEVER use Agent tool for cron tools | ✓ |
| No — design is implicit enough | Skip the explicit rule | |

**User's choice:** Yes — add a prominent CRITICAL constraint before the reconciliation algorithm.
