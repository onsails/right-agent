---
id: SEED-005
status: dormant
planted: 2026-03-23
planted_during: v1.0 / manual testing
trigger_when: next milestone or skill management phase
scope: Medium
---

# SEED-005: Support skills.sh (Vercel) instead of or alongside ClawHub

## Problem

RightClaw's `/clawhub` skill targets the ClawHub registry (clawhub.ai) specifically. But ClawHub is:
- OpenClaw-specific with proprietary extensions (`metadata.openclaw`)
- Had a major security incident (ClawHavoc, Feb 2026 — 341 malicious skills, 12% of registry)
- One of 6+ registries, not the universal one

Meanwhile, **skills.sh** (by Vercel) has emerged as the de facto distribution hub:
- Uses the standard agentskills.io SKILL.md format (no proprietary extensions)
- ~11.4k GitHub stars, 200+ curated skills
- Cross-agent: works with Claude Code, Codex, Cursor, Gemini CLI, etc.
- CLI: `npx skills add <owner>/<repo>` — simple, familiar
- GitHub-native: no separate publish step, telemetry-based leaderboard

## Proposed changes

1. **Rename `/clawhub` skill to `/skills`** — generic skill manager, not tied to one registry
2. **Support skills.sh as primary** — `search`, `install`, `remove` via skills.sh API
3. **Keep ClawHub as secondary** — for backward compatibility with OpenClaw ecosystem
4. **Add tech-leads-club as verified source** — security-hardened, Snyk-scanned skills
5. **Policy gate works with any source** — audit `metadata` frontmatter regardless of registry

## The registry landscape (March 2026)

| Registry | Skills | Approach | Security |
|----------|--------|----------|----------|
| skills.sh (Vercel) | 200+ curated | GitHub-native, telemetry leaderboard | None built-in |
| ClawHub | 13,729+ | Vector search, OpenClaw ecosystem | VirusTotal (post-ClawHavoc) |
| claude-plugins.dev | 51,625+ | Auto-indexed from GitHub | None |
| SkillsMP.com | 500,000+ | Semantic search marketplace | Basic quality filters |
| Tech Leads Club | Small, curated | Human-curated, CI/CD verified | Snyk scan, content hashing |
| anthropics/skills | ~40 reference | Official Anthropic examples | Anthropic-maintained |

## Breadcrumbs

- `skills/clawhub/SKILL.md` — current skill (references clawhub.ai API)
- `skills/cronsync/SKILL.md` — not affected (agent-local skill)
- agentskills.io spec: https://agentskills.io/specification
- skills.sh: https://skills.sh/
- ClawHub security incident: Snyk ToxicSkills study

## Scope estimate

Medium — rewrite the `/clawhub` SKILL.md to support multiple registries, update install paths to `.claude/skills/`.
