# RightClaw

## What This Is

RightClaw is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs as an independent Claude Code session inside its own OpenShell sandbox with declarative YAML policies. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

## Core Value

Run multiple autonomous Claude Code agents safely — each sandboxed by OpenShell policies, each with its own identity and memory, orchestrated by a single CLI command.

## Requirements

### Validated

- ✓ Rust project with edition 2024, Cargo workspace, devenv — Phase 1
- ✓ Agent directory structure follows OpenClaw conventions — Phase 1
- ✓ Agent discovery and validation (IDENTITY.md + policy.yaml required) — Phase 1
- ✓ Per-agent agent.yaml config with deny_unknown_fields — Phase 1
- ✓ `rightclaw init` creates ~/.rightclaw/ + default agent — Phase 1
- ✓ `rightclaw list` shows discovered agents — Phase 1
- ✓ `rightclaw up` generates wrappers + PC config, launches agents in OpenShell sandboxes — Phase 2
- ✓ `rightclaw up --agents`, `up -d`, `down`, `status`, `restart`, `attach` — Phase 2
- ✓ Per-agent shell wrapper with OpenShell sandbox enforcement — Phase 2
- ✓ process-compose REST API integration via Unix socket — Phase 2
- ✓ Explicit sandbox destroy on shutdown — Phase 2
- ✓ Default "Right" agent with BOOTSTRAP.md onboarding (name, creature, vibe, emoji) — Phase 3
- ✓ Production OpenShell policy.yaml with hard_requirement Landlock + comprehensive comments — Phase 3
- ✓ install.sh one-liner with platform detection + dependency installation — Phase 3
- ✓ `rightclaw doctor` validates dependencies and agent structure — Phase 3
- ✓ Telegram channel setup via `rightclaw init --telegram-token` — Phase 3
- ✓ Shell wrapper conditional `--channels` flag for Telegram — Phase 3
- ✓ `/clawhub` skill — search, install, remove, list via ClawHub HTTP API with policy gate — Phase 4
- ✓ `/cronsync` skill — declarative cron reconciliation with lock-file concurrency — Phase 4
- ✓ System prompt codegen for CronSync bootstrap — Phase 4

### Active

(All v1 requirements validated — see above)

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
| Rust for CLI | User preference, performance, type safety | ✓ Good |
| process-compose for orchestration | No need to build our own process manager, TUI comes free | ✓ Good |
| OpenShell for sandboxing | Official NVIDIA solution, declarative policies, kernel-level enforcement | ✓ Good |
| Drop-in OpenClaw compatibility | Access to 5,700+ existing ClawHub skills and established conventions | ✓ Good |
| ClawHub via HTTP API (no CLI dep) | Fewer dependencies, more control over UX | ✓ Good |
| One default agent ("Right") | Ship the runtime with a working example, not 5 half-baked agents | ✓ Good |
| CronSync as Claude Code skill | Cron management happens inside CC sessions, not CLI concern | ✓ Good |
| System-level tool (~/.rightclaw/) | No project-path argument, agents are global | ✓ Good |
| Agent dir as cwd | CC reads SOUL.md/AGENTS.md naturally from cwd | ✓ Good |
| Generated system prompt for CronSync | Non-editable, regenerated on each `up` | ✓ Good |

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

## Current State

**v1.0 shipped** (2026-03-23). Core runtime works:
- `rightclaw init/up/down/status/attach` — full CLI lifecycle
- Telegram channels — auto-pairing, messaging, cron job output
- Skills management via skills.sh (agentskills.io format)
- RightCron — declarative cron reconciliation with bootstrap on startup
- OpenShell sandbox support (requires API key for auth)

**Known limitations (seeds for v2):**
- BOOTSTRAP.md onboarding doesn't trigger via Telegram (SEED-002)
- OpenShell sandbox requires ANTHROPIC_API_KEY, no OAuth (SEED-003)
- Host settings leak into agent sessions (SEED-004)
- `rightclaw restart` disabled (process-compose bug with is_tty)

---
*Last updated: 2026-03-23 after v1.0 milestone completion*
