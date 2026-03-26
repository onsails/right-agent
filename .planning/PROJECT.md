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
- ✓ Per-agent `memory.db` (SQLite, WAL mode) created on `rightclaw up`; V1 schema with `memories` + `memory_events` (append-only, ABORT triggers) + FTS5 virtual table; rusqlite_migration 2.5 — v2.3 Phase 16
- ✓ `memory_path` field removed from `AgentDef`; MEMORY.md no longer referenced in codebase (CC manages it natively); default start_prompt updated to `"You are starting."` — v2.3 Phase 16
- ✓ `rightclaw doctor` warns (non-fatal) when `sqlite3` binary absent from PATH — v2.3 Phase 16
- ✓ `rightclaw memory-server` subcommand: rmcp 1.3 stdio MCP server exposing store/recall/search/forget tools backed by per-agent SQLite — v2.3 Phase 17 (SKILL-01..04)
- ✓ `cmd_up` generates per-agent `.mcp.json` with `mcpServers.rightmemory` entry on every `rightclaw up` — v2.3 Phase 17 (SKILL-05)
- ✓ `store_memory` rejects content matching 15 OWASP-derived injection patterns via `guard::has_injection` — v2.3 Phase 17 (SEC-01)

### Active

- ✓ `/skills` skill replaces `/clawhub` — skills.sh (Vercel) as primary registry; ClawHub removed completely — v2.2 Phase 12
- ✓ `rightclaw up` removes stale `.claude/skills/clawhub/` from existing agent dirs — v2.2 Phase 12
- ✓ `SKILL_CLAWHUB` renamed to `SKILL_SKILLS`; init/up installs `skills/SKILL.md` — v2.2 Phase 12
- ✓ Policy gate reworked — reads `compatibility` prose, BLOCKs on sandbox mismatches, WARNs on missing bins/env; `/skill-doctor` command added — v2.2 Phase 13
- ✓ `env:` section in `agent.yaml` — per-agent env var injection (single-quoted, no expansion) — v2.2 Phase 11
- ✓ Shell wrapper exports `env:` vars after identity captures, before `export HOME=` — v2.2 Phase 11
- ✓ `install_builtin_skills()` uses create-if-absent for `installed.json` — survives restarts — v2.2 Phase 11
- ✓ `/skills` skill renamed to `/rightskills`; source dir `skills/rightskills/`, constant `SKILL_RIGHTSKILLS`, install path `rightskills/SKILL.md` — v2.2 Phase 14
- ✓ `cmd_up` removes stale `.claude/skills/skills/` dir on upgrade; SUMMARY.md frontmatter gaps resolved — v2.2 Phase 15

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

**v2.2 shipped** (2026-03-26). Skills Registry complete — ClawHub removed, `/rightskills` (skills.sh) installed as built-in, per-agent env var injection via `agent.yaml`, CC-native policy gate with BLOCK/WARN two-tier checking, `/skill-doctor` audit command. 18/18 requirements satisfied across 5 phases. [Full archive](milestones/v2.2-ROADMAP.md)

**Shipped versions:**
- v1.0 (2026-03-23): Core runtime — CLI, process-compose, OpenShell sandbox, Telegram, skills, RightCron
- v2.0 (2026-03-24): Native sandbox — replaced OpenShell with CC sandbox (bubblewrap/Seatbelt), per-agent settings.json, SandboxOverrides in agent.yaml, doctor AppArmor smoke test
- v2.1 (2026-03-25): Headless agent isolation — per-agent HOME override + credential symlinks + git/SSH forwarding, pre-populated .claude/ scaffold (settings.json, settings.local.json, skills/), git init, Telegram channel copy, managed-settings doctor check
- v2.2 (2026-03-26): Skills registry — ClawHub removed, `/rightskills` (skills.sh) as built-in, per-agent env var injection, CC-native policy gate, `/skill-doctor` command

**Known limitations:**
- SEED-002: BOOTSTRAP.md onboarding doesn't trigger via Telegram
- `rightclaw restart` status unknown — changed `is_tty` to `is_interactive` (correct field name); restart may now work
- `test_status_no_running_instance` integration test fails (pre-existing)
- Tech debt: git absence warning in `verify_dependencies()` (called by `rightclaw up`) but not surfaced by `rightclaw doctor`

## Current Milestone: v2.3 Memory System

**Goal:** Give each agent a per-agent SQLite-backed memory store — persistent across restarts, queryable via a built-in skill, and inspectable from the CLI.

**Target features:**
- Per-agent SQLite database (lives in agent HOME dir, no cross-agent sharing)
- Built-in memory skill: store, recall, search, forget operations
- `rightclaw memory` CLI subcommand: list, search, delete
- Full audit trail: timestamps + provenance on every entry

---
*Last updated: 2026-03-26 — v2.3 Phase 17 complete — Memory Skill shipped (MCP store/recall/search/forget, injection guard, .mcp.json codegen)*
