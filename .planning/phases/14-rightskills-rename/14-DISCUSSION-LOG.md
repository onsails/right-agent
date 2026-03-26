# Phase 14: rightskills Rename - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-26
**Phase:** 14-rightskills-rename
**Areas discussed:** skill-doctor command name, SKILL.md content changes, Stale dir cleanup

---

## skill-doctor command name

| Option | Description | Selected |
|--------|-------------|----------|
| Keep /skill-doctor | Section stays `### skill-doctor`. Descriptive and standalone. | ✓ |
| Rename to /rightskills-doctor | Namespace-consistent with the skill name | |
| Rename to /doctor | Short, risk of clash with other skills | |

**User's choice:** Keep `/skill-doctor`
**Notes:** No change to the section name or invocation.

---

## SKILL.md content changes

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — update all invocation examples | Replace `/skills install`, `/skills list`, etc. → `/rightskills install`, `/rightskills list` throughout body | ✓ |
| Only the name field — leave body as-is | Minimal diff but stale/wrong body text | |

**User's choice:** Yes — update all invocation examples
**Notes:** Only slash command references change. `skills.sh` domain names must NOT be touched.

---

## Stale dir cleanup

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — remove stale skills/ dir | `remove_dir_all(.claude/skills/skills)` in cmd_up, same as clawhub pattern | |
| No — leave old dir in place | Untidy but harmless | |
| Not needed (user choice) | Not in prod yet — no cleanup needed | ✓ |

**User's choice:** No stale cleanup
**Notes:** "we are not in prod yet, so we don't need this cleanup"

---

## Claude's Discretion

- Exact substitution strategy for body text replacements (careful not to touch `skills.sh`)
- Whether to use `git mv` or plain filesystem rename for the directory

## Deferred Ideas

- Stale `.claude/skills/skills/` cleanup — deferred explicitly (not in prod)
- `rightclaw up` / `rightclaw doctor` frontmatter validation — Phase 15+
