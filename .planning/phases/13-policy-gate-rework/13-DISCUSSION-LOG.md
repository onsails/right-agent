# Phase 13: Policy Gate Rework - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-26
**Phase:** 13-policy-gate-rework
**Areas discussed:** Scope, Skill requirement declaration, Bins/env checks, npx domain handling, skill-doctor design

---

## Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Policy gate + skill-doctor (original + skill-doctor) | Rewrite gate, add skill-doctor command to SKILL.md | ✓ |
| Rename + policy gate + skill-doctor | All in one phase including rightskills rename | |
| Only policy gate (original scope) | Strict scope, rename and skill-doctor separate | |

**User's choice:** Policy gate + skill-doctor
**Notes:** `rightskills` rename deferred to Phase 14. CLI validation (rightclaw up/doctor) deferred to Phase 15+.

---

## Frontmatter Standard Research

| Option | Description | Selected |
|--------|-------------|----------|
| Interpret compatibility prose + check settings.json | Gate reads compatibility field, Claude interprets requirements | ✓ |
| Ignore compatibility, check only settings.json | Skip skill-declared requirements entirely | |
| Require structured metadata.requires.* | Non-standard but predictable | |

**User's choice:** After pushing back on custom frontmatter, user directed research into skills.sh/agentskills.io/pinchtab. Research conclusively showed NO structured requirement fields exist in the standard. The `compatibility` prose field is the official channel.

**Research findings:** agentskills.io defines: name, description, license, compatibility (prose), metadata (arbitrary), allowed-tools (experimental). No `requires.*` structured fields anywhere in the ecosystem. skills.sh uses the same standard. `metadata.openclaw.requires.*` was a non-standard OpenClaw extension.

---

## Bins/env Checks

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, as advisory pre-flight | `which` for bins, env check, warn only | ✓ |
| No, drop them entirely | Check only sandbox-enforced caps | |
| Check only in skill-doctor | Install gate lean, skill-doctor does full check | |

**User's choice:** Advisory checks at install time AND in skill-doctor.
**Notes:** User initially asked "filesystem IS the bins, no?" — clarified that bin availability (which/PATH) is separate from sandbox filesystem (allowWrite/denyRead). CC sandbox doesn't restrict /usr/bin etc. by default.

---

## npx Domain Handling

| Option | Description | Selected |
|--------|-------------|----------|
| Warn before install if domains missing | Check skills.sh/npmjs.org against settings.json | ✓ |
| Add note to SKILL.md only | Document, no runtime check | |

**User's choice:** Warn if skills.sh/npmjs.org not in allowedDomains.
**Notes:** DEFAULT_ALLOWED_DOMAINS in settings.rs includes `npmjs.org` but NOT `skills.sh`. Both should be checked.

---

## skill-doctor Checks

| Option | Description | Selected |
|--------|-------------|----------|
| All installed skills compatibility | ✓ | ✓ |
| Bins advisory check | ✓ | ✓ |
| Network/domains vs settings.json | ✓ | ✓ |
| Env vars check | ✓ | ✓ |

**User's choice:** All four checks.

---

## skill-doctor Output Format

| Option | Description | Selected |
|--------|-------------|----------|
| Table: skill vs capability vs status | One row per skill | ✓ |
| Grouped by issue type | Sections per issue type | |
| Pass/fail per skill with details | Each skill PASS or FAIL | |

**User's choice:** Table format.

---

## Claude's Discretion

- How to parse requirements from compatibility prose (Claude interprets naturally)
- Exact block/warn message wording
- Whether skill-doctor includes on-disk skills not in installed.json

## Deferred Ideas

- `rightskills` rename — Phase 14
- `rightclaw up` / `rightclaw doctor` skill validation — Phase 15+
- Structured `metadata.requires.*` — research showed the ecosystem uses prose only; revisit if skills.sh adds a standard extension
