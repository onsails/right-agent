# RightClaw

## What This Is

RightClaw is a multi-agent runtime for Claude Code. Each agent runs as an independent Claude Code session with native OS-level sandboxing (bubblewrap/Seatbelt) and per-agent sandbox configuration. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

## Core Value

Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own sandbox configuration and identity, orchestrated by a single CLI command.

## Requirements

### Validated

- ✓ Rust project with edition 2024, Cargo workspace, devenv — Phase 1
- ✓ Agent directory structure follows OpenClaw conventions — Phase 1
- ✓ Agent discovery and validation (IDENTITY.md required, policy.yaml removed Phase 5) — Phase 1
- ✓ Per-agent agent.yaml config with deny_unknown_fields — Phase 1
- ✓ `rightclaw init` creates ~/.rightclaw/ + default agent — Phase 1
- ✓ `rightclaw list` shows discovered agents — Phase 1
- ✓ `rightclaw up` generates wrappers + PC config, launches agents directly — Phase 2 (OpenShell removed Phase 5)
- ✓ `rightclaw up --agents`, `up -d`, `down`, `status`, `restart`, `attach` — Phase 2
- ✓ Per-agent shell wrapper with direct claude invocation — Phase 2 (OpenShell removed Phase 5)
- ✓ process-compose REST API integration via Unix socket — Phase 2
- ✓ Default "Right" agent with BOOTSTRAP.md onboarding (name, creature, vibe, emoji) — Phase 3
- ~~Production OpenShell policy.yaml~~ — removed Phase 5 (replaced by CC native sandbox)
- ✓ install.sh one-liner with platform detection + dependency installation — Phase 3
- ✓ `rightclaw doctor` validates dependencies and agent structure — Phase 3
- ✓ Telegram channel setup via `rightclaw init --telegram-token` — Phase 3
- ✓ Shell wrapper conditional `--channels` flag for Telegram — Phase 3
- ✓ `/clawhub` skill — search, install, remove, list via ClawHub HTTP API with policy gate — Phase 4
- ✓ `/cronsync` skill — declarative cron reconciliation with lock-file concurrency — Phase 4
- ✓ System prompt codegen for CronSync bootstrap — Phase 4
- ✓ OpenShell removed, agents launch via direct claude invocation — v2.0 Phase 5
- ✓ Per-agent `.claude/settings.json` with CC native sandbox config — v2.0 Phase 6
- ✓ SandboxOverrides in agent.yaml for per-agent customization — v2.0 Phase 6
- ✓ Doctor checks bubblewrap/socat on Linux with AppArmor smoke test — v2.0 Phase 7
- ✓ install.sh installs bubblewrap/socat (apt/dnf/pacman) — v2.0 Phase 7
- ✓ Shell wrapper sets HOME=$AGENT_DIR + forwards 6 identity env vars before HOME override — v2.1 Phase 8
- ✓ Per-agent .claude.json with hasTrustDialogAccepted generated on every `up`/`init` — v2.1 Phase 8
- ✓ Credential symlink $AGENT_DIR/.claude/.credentials.json → host OAuth creds — v2.1 Phase 8
- ✓ denyRead uses absolute host HOME paths (not tilde); allowRead includes agent path — v2.1 Phase 8
- ✓ SandboxOverrides.allow_read for per-agent allowRead overrides — v2.1 Phase 8
- ✓ `rightclaw up` runs `git init` in each agent dir that lacks .git/ (non-fatal) — v2.1 Phase 9
- ✓ `rightclaw up` writes Telegram channel config per-agent when telegram fields set in agent.yaml — v2.1 Phase 9
- ✓ `rightclaw up` reinstalls built-in skills into each agent's .claude/skills/ on every launch — v2.1 Phase 9
- ✓ `rightclaw up` writes settings.local.json with {} if absent, never overwrites existing — v2.1 Phase 9
- ✓ `rightclaw doctor` warns (non-fatal) when git binary absent — v2.1 Phase 9
- ✓ `rightclaw config strict-sandbox` writes `/etc/claude-code/managed-settings.json` with `allowManagedDomainsOnly: true` (requires sudo) — v2.1 Phase 10
- ✓ `rightclaw doctor` detects managed-settings.json and warns if `allowManagedDomainsOnly:true` may conflict with per-agent settings — v2.1 Phase 10

### Active

(No active requirements — run `/gsd:new-milestone` to define next)

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
- **Agent isolation:** Each agent dir (`~/.rightclaw/agents/<name>/`) has its own `.claude/settings.json` generated on every `rightclaw up`. Per-agent sandbox overrides via `agent.yaml` `sandbox:` section.
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

## Current State

**v2.1 Phase 10 complete** (2026-03-25). Doctor & Managed Settings — `rightclaw config strict-sandbox` opt-in command and doctor conflict detection for `/etc/claude-code/managed-settings.json`. 154 lib tests, 20 integration tests.

**Shipped versions:**
- v1.0 (2026-03-23): Core runtime — CLI, process-compose, OpenShell sandbox, Telegram, skills, RightCron
- v2.0 (2026-03-24): Native sandbox — replaced OpenShell with CC sandbox (bubblewrap/Seatbelt), per-agent settings.json, SandboxOverrides in agent.yaml, doctor AppArmor smoke test

**Known limitations:**
- SEED-002: BOOTSTRAP.md onboarding doesn't trigger via Telegram
- `rightclaw restart` disabled (process-compose is_tty bug)
- `test_status_no_running_instance` integration test fails (pre-existing)

## Current Milestone: v2.1 Headless Agent Isolation

**Goal:** Make agents fully autonomous without interactive TUI prompts — complete isolation from host config, silent sandbox enforcement, no bypass warnings.

**Target features:**
- Drop `--dangerously-skip-permissions`, use explicit `permissions.allow` + sandbox (no bypass warning)
- Managed settings for `allowManagedDomainsOnly: true` (silent domain blocking)
- Per-agent `$HOME` override — agents don't see host config
- Solve HOME edge cases: trust file, OAuth/API key, git/SSH env forwarding

---
*Last updated: 2026-03-25 — Phase 10 complete (doctor-managed-settings)*
