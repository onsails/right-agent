# RightClaw

## What This Is

RightClaw is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs as an independent Claude Code session inside its own OpenShell sandbox with declarative YAML policies. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

## Core Value

Run multiple autonomous Claude Code agents safely — each sandboxed by OpenShell policies, each with its own identity and memory, orchestrated by a single CLI command.

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] Rust CLI (`rightclaw`) that manages agent lifecycles via process-compose
- [ ] `rightclaw up` scans `agents/`, generates process-compose.yaml, launches agents
- [ ] `rightclaw up --agents <list>` to launch specific agents only
- [ ] `rightclaw up -d` for detached mode (background with TUI server)
- [ ] `rightclaw attach` to connect to running TUI
- [ ] `rightclaw status` to show agent states
- [ ] `rightclaw restart <agent>` to restart a single agent
- [ ] `rightclaw down` to stop all agents
- [ ] Each agent launched inside OpenShell sandbox (`openshell sandbox create --policy <policy> -- claude`)
- [ ] Shell wrapper per agent: extracts policy from agent dir, wraps `openshell sandbox create` invocation
- [ ] OpenShell policy YAML per agent (filesystem, network, process restrictions)
- [ ] Agent directory structure matches OpenClaw conventions (SOUL.md, USER.md, IDENTITY.md, MEMORY.md, AGENTS.md, TOOLS.md, BOOTSTRAP.md, HEARTBEAT.md)
- [ ] Default "Right" agent — general-purpose with onboarding flow (asks name, vibe, personality)
- [ ] BOOTSTRAP.md for "Right" agent that runs on first conversation, writes IDENTITY.md/USER.md/SOUL.md, then self-deletes
- [ ] CronSync as a Claude Code skill — reconciles `crons/*.yaml` specs with live CronCreate/CronList/CronDelete
- [ ] Lock-file concurrency control for cron jobs (heartbeat-based, UTC ISO 8601)
- [ ] `/clawhub` Claude Code skill — install, uninstall, list, search ClawHub skills via ClawHub HTTP API
- [ ] Policy gate for installed skills — audit permissions before activation
- [ ] `install.sh` script that installs RightClaw CLI, process-compose, and OpenShell
- [ ] Per-agent `agent.yaml` for restart policy, backoff, custom start prompt (optional, defaults apply)
- [ ] Per-agent `.mcp.json` for MCP server configuration
- [ ] Rust devenv configuration (edition 2024)

### Out of Scope

- Shared memory between agents (phase 2 — MCP memory server)
- Building specific task agents (watchdog, reviewer, scout, ops, forge) — users define their own
- Central orchestrator or master session — agents are autonomous
- Token arbitrage or unofficial API access — only Claude API / legitimate subscription
- Web UI or dashboard — TUI via process-compose is sufficient
- ClawHub registry service itself — we consume it, not build it
- `clawhub` CLI dependency — our skill talks to API directly

## Context

- **Positioning:** RightClaw is the "done right" alternative to OpenClaw. Same ecosystem compatibility (ClawHub skills, file conventions), but with OpenShell sandbox enforcement instead of unrestricted system access.
- **OpenShell:** NVIDIA's open-source sandbox runtime (Apache 2.0, released March 2026 at GTC). Uses Landlock LSM + Seccomp BPF on Linux, Docker Desktop on macOS. Declarative YAML policies for filesystem, network, process, and inference restrictions. Alpha status, single-player mode.
- **OpenClaw ecosystem:** ~5,700 ClawHub skills, SKILL.md format with YAML frontmatter, `metadata.openclaw` for gating. Agent files: SOUL.md (personality/values), USER.md (user context), IDENTITY.md (name/vibe/emoji), MEMORY.md (persistent facts), AGENTS.md (operational framework), BOOTSTRAP.md (first-run onboarding, self-deletes).
- **process-compose:** Lightweight process orchestrator with TUI. Handles restart policies, logging, process groups. RightClaw generates its config, doesn't ship its own process manager.
- **CronSync:** Built as a Claude Code skill (not CLI concern). Uses Claude Code's native CronCreate/CronList/CronDelete tools. Declarative YAML specs in `agents/<name>/crons/`, reconciled via `/loop`. Lock files with heartbeat for concurrency control.
- **Name origin:** RightClaw = doing the claw (agent) right. Right claw is precise, surgical. Antithesis to OpenClaw's "grab everything" approach. Product of onsails studio.

## Constraints

- **Language**: Rust (edition 2024)
- **Dependencies**: process-compose (external), OpenShell (external), Claude Code CLI (external)
- **Platforms**: Linux and macOS
- **Compatibility**: Drop-in compatible with OpenClaw file conventions and ClawHub SKILL.md format
- **Security**: Every agent must run inside OpenShell sandbox — no `--dangerously-skip-permissions` without policy enforcement
- **OpenShell status**: Alpha software — may have breaking changes. Design for resilience.

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Rust for CLI | User preference, performance, type safety | — Pending |
| process-compose for orchestration | No need to build our own process manager, TUI comes free | — Pending |
| OpenShell for sandboxing | Official NVIDIA solution, declarative policies, kernel-level enforcement | — Pending |
| Drop-in OpenClaw compatibility | Access to 5,700+ existing ClawHub skills and established conventions | — Pending |
| ClawHub via HTTP API (no CLI dep) | Fewer dependencies, more control over UX | — Pending |
| One default agent ("Right") | Ship the runtime with a working example, not 5 half-baked agents | — Pending |
| CronSync as Claude Code skill | Cron management happens inside CC sessions, not CLI concern | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd:transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-03-21 after initialization*
