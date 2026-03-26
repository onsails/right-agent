---
phase: 13-policy-gate-rework
plan: "01"
subsystem: skills
tags: [skill-manager, policy-gate, sandbox, skill-doctor]
dependency_graph:
  requires: []
  provides: [GATE-01, GATE-02]
  affects: [skills/skills/SKILL.md]
tech_stack:
  added: []
  patterns: [two-tier BLOCK/WARN gate, prose-based compatibility parsing, bash-block CLI ops]
key_files:
  created: []
  modified:
    - skills/skills/SKILL.md
decisions:
  - "Replaced Step 3 in full — no patching of metadata.openclaw refs, clean rewrite"
  - "BLOCK only on network domain/filesystem write sandbox mismatches; WARN on bins/env"
  - "Pre-install check (D-06) placed before npx command in Step 2, distinct from post-install gate"
  - "skill-doctor unions installed.json + disk scan; untracked skills labeled manual"
  - "skill-doctor table has 6 columns: Skill/Source/Bins/Env Vars/Network/Status"
metrics:
  duration: "2 minutes"
  completed: "2026-03-26"
  tasks_completed: 2
  files_modified: 1
requirements-completed: [GATE-01, GATE-02]
---

# Phase 13 Plan 01: Policy Gate Rework Summary

**One-liner:** Rewrote `/skills` SKILL.md policy gate — replaced `metadata.openclaw.requires.*` refs with CC-native sandbox `compatibility` prose gate, added pre-install domain check, and new `skill-doctor` audit command.

## What Changed in SKILL.md

### Sections Modified

**Frontmatter**
- Added `compatibility: Requires Node.js (npx), internet access to skills.sh and npmjs.org` (D-07)

**`### install <slug>` — Step 2**
- Added "Pre-install domain check" block before `npx skills add` command
- Reads `.claude/settings.json`, checks `sandbox.network.allowedDomains` for `skills.sh` and `registry.npmjs.org`
- Exact D-06 warn message: "Warning: `skills.sh` is not in your sandbox `allowedDomains`. Add it to your `agent.yaml` sandbox overrides: `allowed_domains: [skills.sh, registry.npmjs.org]`"

**`### install <slug>` — Step 3 (full replacement)**
- Dropped all `metadata.openclaw.requires.*` references (GATE-01)
- New gate reads `compatibility` prose field from downloaded skill's SKILL.md
- Natural-language classification: bins (executables), env vars (ALL_CAPS tokens), network (domain-like tokens), filesystem write (phrase matching)
- Two-tier gate: BLOCK on network/filesystem sandbox mismatches; WARN only on missing bins/env
- References `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` in `.claude/settings.json` (GATE-02)
- Audit table output before proceed/block decision

**`## Important Rules` — Rule 2**
- Updated to explicitly name both the pre-install domain check (Step 2) and policy gate audit (Step 3)

### Sections Added

**`### skill-doctor`** (new section between `### update` and `## Error Handling`)
- Two-source skill discovery: `installed.json` (tracked) + disk scan of `.claude/skills/*/SKILL.md` (untracked labeled `manual`)
- Reads `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` from `.claude/settings.json`
- Per-skill compatibility check: bins (`which <bin>`), env vars (`printenv <VAR>`), network/filesystem vs sandbox config
- No-`compatibility`-field handling: marks as `—` / PASS
- Output table: Skill | Source | Bins | Env Vars | Network | Status (6 columns)
- PASS/WARN/BLOCK status rules matching install gate semantics
- Post-table actionable summary for BLOCK and WARN items

## Verification Results

```
# GATE-01: No OpenShell references
rg "metadata.openclaw|policy.yaml|OpenShell" skills/skills/SKILL.md | wc -l
→ 0

# GATE-02: settings.json field references present
rg "allowedDomains|allowWrite" skills/skills/SKILL.md
→ 9 matches across Step 2, Step 3, and skill-doctor

# D-07: compatibility frontmatter
rg "^compatibility:" skills/skills/SKILL.md
→ compatibility: Requires Node.js (npx), internet access to skills.sh and npmjs.org

# D-06: pre-install domain warn message
rg "skills.sh.*not in your sandbox" skills/skills/SKILL.md
→ 1 match (exact CONTEXT.md wording)

# skill-doctor command present
rg "### skill-doctor" skills/skills/SKILL.md
→ 1 match, at line 251 (before ## Error Handling at line 319)

# File structure preserved
rg "^##|^###" skills/skills/SKILL.md
→ ## When to Activate, ## Configuration, ## Commands,
   ### search, ### install, ### remove, ### list, ### update,
   ### skill-doctor, ## Error Handling, ## Important Rules
```

## Discretion Choices Made

1. **Compatibility prose interpretation guidance** — Added explicit classification heuristics to both Step 3 and skill-doctor: bins = named executables, env vars = ALL_CAPS_WITH_UNDERSCORES non-domain tokens, network = dot-containing tokens, filesystem = phrase matching. This gives Claude enough signal to classify consistently without structured parsing.

2. **skill-doctor table columns** — Plan specified 6 columns (Skill/Source/Bins/Env Vars/Network/Status). CONTEXT.md §Specifics showed 5 columns without Source. Used the plan's 6-column version per the acceptance criteria requirement.

3. **No-`compatibility` field handling** — Documented explicitly in both Step 3 and skill-doctor: treat absence as no requirements → PASS. Per research open question recommendation.

4. **registry.npmjs.org placement** — Both in D-06 warn message (as `registry.npmjs.org`) and as an example in network classification guidance. Addresses pitfall 4 (subdomain wildcard uncertainty).

## Deviations from Plan

None — plan executed exactly as written. All four edits in Task 1 and the skill-doctor insertion in Task 2 completed cleanly.

## Commits

| Task | Commit | Message |
|------|--------|---------|
| Task 1 | `2d59c8a` | feat(13-01): rewrite frontmatter, Step 2 domain pre-check, Step 3 policy gate |
| Task 2 | `afe8154` | feat(13-01): add skill-doctor command section |

## Self-Check: PASSED
