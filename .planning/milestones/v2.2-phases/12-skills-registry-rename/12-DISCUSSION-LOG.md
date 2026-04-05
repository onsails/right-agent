# Phase 12: Skills Registry Rename - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-25
**Phase:** 12-skills-registry-rename
**Areas discussed:** Stale dir cleanup, Policy gate scope

---

## Stale Dir Cleanup

| Option | Description | Selected |
|--------|-------------|----------|
| Silent delete, ignore errors | `fs::remove_dir_all().ok()` — non-fatal, no logging | ✓ |
| Logged cleanup | Print message to user per cleaned agent | |
| FAIL FAST | Propagate error if removal fails | |

**User's choice:** Silent delete, ignore errors
**Notes:** "We are not in prod. Just don't care about stale dirs." — keep it simple, non-fatal.

---

## Policy Gate Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Defer to Phase 13 | Leave `metadata.openclaw.requires` table in SKILL.md, Phase 13 rewrites entirely | ✓ |
| Strip the table now | Remove OpenShell refs in Phase 12, Phase 13 adds CC-native logic | |

**User's choice:** Defer to Phase 13
**Notes:** User asked what Phase 13 was — once explained (full policy gate rewrite), deferred cleanly. No point doing half the work twice.

---

## Claude's Discretion

- Whether to guard stale cleanup with `path.exists()` check or just `remove_dir_all().ok()`
- Plan count (likely one — all mechanical rename)

## Deferred Ideas

- `metadata.openclaw.requires` cleanup → Phase 13 (GATE-01, GATE-02)
