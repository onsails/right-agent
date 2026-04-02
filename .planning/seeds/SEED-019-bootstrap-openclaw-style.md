---
id: SEED-019
status: dormant
planted: 2026-04-02
planted_during: v3.0 Teloxide Bot Runtime (phase 28.2 UAT)
trigger_when: Next milestone — any work on agent templates, onboarding, or identity setup
scope: small
---

# SEED-019: Rewrite BOOTSTRAP.md to match OpenClaw onboarding conventions

## Why This Matters

The current `templates/right/BOOTSTRAP.md` is rightclaw-specific and overly narrative
("You just woke up. Blank slate..."). OpenClaw has established a standard first-boot
onboarding flow for filling IDENTITY.md, SOUL.md, and related files — agents in the
ecosystem should follow the same conventions for compatibility and familiarity.

A BOOTSTRAP.md that follows OpenClaw conventions means:
- Agents from OpenClaw/ClawHub work the same way in rightclaw
- Users who know OpenClaw aren't surprised
- Drop-in compatibility claim is actually true for onboarding too

## When to Surface

**Trigger:** Next milestone — surface whenever touching agent templates, BOOTSTRAP.md,
IDENTITY.md, SOUL.md, or onboarding flow.

## Scope Estimate

**Small** — Review OpenClaw's BOOTSTRAP.md format, rewrite `templates/right/BOOTSTRAP.md`
to match. Also update BOOTSTRAP.md to mention:
- Available memory tools (see SEED-018)
- Available skills (rightskills, rightcron)

## Breadcrumbs

- `templates/right/BOOTSTRAP.md` — current rightclaw-specific version to replace
- `templates/right/IDENTITY.md` — what the agent fills in during bootstrap
- `templates/right/SOUL.md` — what gets customised during bootstrap
- `templates/right/USER.md` — user profile written during bootstrap
- OpenClaw reference: https://github.com/onsails/openclaw (check BOOTSTRAP.md format there)

## Notes

Current BOOTSTRAP.md flow is fine conceptually (name → nature → vibe → emoji → write files
→ delete self) but the format/wording may diverge from OpenClaw standard. Check OpenClaw
repo for canonical format before rewriting. Don't invent a new format — just align.

Also consider: should the bootstrap prompt mention `mcp__rightclaw__store` to save the
agent's name/vibe to memory on first boot? (Connects to SEED-018.)
