---
id: SEED-009
status: dormant
planted: 2026-03-27
planted_during: v2.3 Memory System
trigger_when: public release prep or open-sourcing
scope: Small
---

# SEED-009: Comprehensive README with feature showcase and OpenClaw differentiation

## Why This Matters

RightClaw has no README. For public release or open-sourcing, a clear README is the single most important artifact — it's what determines whether someone tries the tool or bounces. The README needs to communicate:

1. **What RightClaw does** — multi-agent Claude Code runtime with real sandboxing
2. **Why it exists** — OpenClaw grants all permissions and prays; RightClaw enforces security-first via CC native sandbox (bubblewrap/Seatbelt)
3. **How it works** — declarative YAML agents, process-compose orchestration, one CLI command

## When to Surface

**Trigger:** When preparing for first public release or open-sourcing

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Public release, launch, or open-sourcing milestone
- Documentation or developer experience milestone
- Community/adoption-focused milestone

## Key Features to Document

- **Declarative agents** — `agent.yaml` defines identity, sandbox overrides, model, skills, channels
- **CC native sandboxing** — bubblewrap (Linux) / Seatbelt (macOS), per-agent `.claude/settings.json` generated from agent.yaml
- **process-compose orchestration** — TUI dashboard, REST API, per-agent logs
- **Agent identity** — IDENTITY.md / SOUL.md / AGENTS.md per agent directory
- **Telegram channels** — per-agent Telegram pairing and access control
- **Skills support** — `.claude/skills/` compatible with agentskills.io SKILL.md format
- **Doctor command** — pre-flight checks for dependencies and sandbox support
- **Init scaffolding** — `rightclaw init` bootstraps agent directory structure
- **No OpenShell dependency** — v2.0 moved to CC native sandbox, no external sandbox tool needed

## OpenClaw Differentiation

| Aspect | RightClaw | OpenClaw |
|--------|-----------|----------|
| Security model | CC native sandbox enforced per-agent | `--dangerously-skip-permissions` with no enforcement |
| Agent isolation | Per-agent settings.json, sandbox overrides | Shared permissions |
| Orchestration | process-compose (TUI, REST API, logs) | Manual or basic scripts |
| Configuration | Declarative YAML (`agent.yaml`) | Convention-based |
| Compatibility | Drop-in OpenClaw/ClawHub file conventions | Native |

## Scope Estimate

**Small** — Document what exists. Standard README sections (overview, quickstart, features, comparison, installation). A few hours of focused work.

## Breadcrumbs

Related code and decisions found in the current codebase:

- `CLAUDE.md` — project description and tech stack (source of truth for features)
- `crates/rightclaw/src/agent/types.rs` — agent.yaml schema and SandboxOverrides
- `crates/rightclaw/src/codegen/settings.rs` — sandbox settings generation
- `crates/rightclaw/src/codegen/settings_tests.rs` — sandbox override test cases
- `crates/rightclaw/src/init.rs` — init scaffolding (agent directory structure)
- `crates/rightclaw-cli/src/main.rs` — CLI commands and subcommands
- `crates/rightclaw/src/codegen/telegram.rs` — Telegram channel integration
- `.planning/PROJECT.md` — project vision and OpenClaw compatibility notes
- `.planning/seeds/SEED-005-skills-sh-instead-of-clawhub.md` — skills ecosystem direction
- `.planning/seeds/SEED-006-rename-clawhub-to-rightskills.md` — registry renaming

## Notes

No README currently exists in the repo. The CLAUDE.md contains a thorough project description that can serve as the starting point. The README should be written for external developers unfamiliar with the project — not as internal documentation.
