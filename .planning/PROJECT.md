# RightClaw

## What This Is

RightClaw is a multi-agent runtime for Claude Code. Each agent runs as an independent Claude Code session with native OS-level sandboxing (bubblewrap/Seatbelt) and full HOME isolation. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

## Core Value

Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own HOME and identity, orchestrated by a single CLI command.

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

(v2.0 requirements — see REQUIREMENTS.md)

### Out of Scope

- Shared memory between agents (future — MCP memory server)
- Building specific task agents (watchdog, reviewer, scout, ops, forge) — users define their own
- Central orchestrator or master session — agents are autonomous
- Token arbitrage or unofficial API access — only Claude API / legitimate subscription
- Web UI or dashboard — TUI via process-compose is sufficient
- ClawHub registry service itself — we consume it, not build it
- `clawhub` CLI dependency — our skill talks to API directly
- OpenShell integration — replaced by CC native sandboxing in v2.0

## Context

- **Positioning:** RightClaw is the "done right" alternative to OpenClaw. Same ecosystem compatibility (ClawHub skills, file conventions), but with sandbox enforcement instead of unrestricted system access.
- **Sandboxing:** Claude Code native sandbox (bubblewrap on Linux, Seatbelt on macOS). OS-level filesystem + network isolation configured via per-agent `settings.json`. Replaced OpenShell in v2.0 — simpler, no API key required, no alpha instability.
- **Agent isolation:** Each agent dir (`~/.rightclaw/agents/<name>/`) is the agent's `$HOME`. CC creates `.claude/` inside it — settings, permissions, memory all naturally scoped per agent.
- **OpenClaw ecosystem:** ~5,700 ClawHub skills, SKILL.md format with YAML frontmatter, `metadata.openclaw` for gating. Agent files: SOUL.md (personality/values), USER.md (user context), IDENTITY.md (name/vibe/emoji), MEMORY.md (persistent facts), AGENTS.md (operational framework), BOOTSTRAP.md (first-run onboarding, self-deletes).
- **process-compose:** Lightweight process orchestrator with TUI. Handles restart policies, logging, process groups. RightClaw generates its config, doesn't ship its own process manager.
- **CronSync:** Built as a Claude Code skill (not CLI concern). Uses Claude Code's native CronCreate/CronList/CronDelete tools. Declarative YAML specs in `agents/<name>/crons/`, reconciled via `/loop`. Lock files with heartbeat for concurrency control.
- **Name origin:** RightClaw = doing the claw (agent) right. Right claw is precise, surgical. Antithesis to OpenClaw's "grab everything" approach. Product of onsails studio.

## Constraints

- **Language**: Rust (edition 2024)
- **Dependencies**: process-compose (external), bubblewrap + socat (Linux sandbox), Claude Code CLI (external)
- **Platforms**: Linux and macOS
- **Compatibility**: Drop-in compatible with OpenClaw file conventions and ClawHub SKILL.md format
- **Security**: Every agent must run with CC native sandbox enabled — per-agent settings.json enforces filesystem + network isolation

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Rust for CLI | User preference, performance, type safety | ✓ Good |
| process-compose for orchestration | No need to build our own process manager, TUI comes free | ✓ Good |
| OpenShell for sandboxing (v1) | Official NVIDIA solution, declarative policies, kernel-level enforcement | Replaced in v2.0 — alpha instability, API key requirement, unnecessary complexity |
| CC native sandbox (v2) | Built into Claude Code, OS-level (bubblewrap/Seatbelt), no extra deps on macOS, no API key | ✓ Good |
| Agent dir as $HOME (v2) | Per-agent isolation without complex config — CC naturally scopes .claude/ per agent | ✓ Good |
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

## Current Milestone: v2.0 Native Sandbox & Agent Isolation

**Goal:** Replace OpenShell with Claude Code's native sandboxing and isolate agents by making each agent directory their `$HOME`.

**Target features:**
- CC native sandbox (bubblewrap/Seatbelt) replaces OpenShell entirely
- Per-agent `$HOME` at `~/.rightclaw/agents/<name>/` — own `.claude/`, settings, memory
- Per-agent `settings.json` generation with `sandbox.*` config (filesystem + network restrictions)
- Remove all OpenShell code paths, policy.yaml handling, sandbox create/destroy
- Update `install.sh` and `rightclaw doctor` — new deps (bubblewrap, socat on Linux), drop openshell
- Update shell wrappers — `HOME=<agent-dir> claude` instead of `openshell sandbox create -- claude`

## Current State

**v1.0 shipped** (2026-03-23). v2.0 in progress.

**v2.0 addresses:**
- SEED-003: OpenShell API key requirement → CC native sandbox needs no API key
- SEED-004: Host settings leak → per-agent HOME isolation

---
*Last updated: 2026-03-23 — milestone v2.0 started*
