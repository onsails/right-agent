# RightClaw

## What This Is

RightClaw is a multi-agent runtime for Claude Code. Each agent runs as an independent Claude Code session with native OS-level sandboxing (bubblewrap/Seatbelt) and per-agent sandbox configuration. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

## Core Value

Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, each with its own sandbox configuration and identity, orchestrated by a single CLI command.

## Current Milestone: v3.0 Teloxide Bot Runtime

**Goal:** Заменить Claude Code channels на per-agent Rust Telegram бот (teloxide), перенести крон в Rust runtime, и дать каждому агенту полный контроль над system prompt.

**Target features:**
- Per-agent teloxide бот процесс управляется через process-compose
- Telegram threads → независимые Claude сессии (thread_id → session_uuid в memory.db)
- `claude -p --session-id / --resume` для stateless взаимодействия с continuity
- System prompt compose из SOUL.md + USER.md + AGENTS.md → `agent/.claude/system-prompt.txt`
- Cron runtime в Rust (tokio task + file watcher), cronsync SKILL.md только для управления файлами

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
- ✓ `rightclaw memory list/search/delete/stats <agent>` CLI subcommands for operator inspection — v2.3 Phase 18 (CLI-01..04)
- ✓ Telegram detection uses `agent.config.telegram_token/telegram_token_file` (not `.mcp.json` presence); `mcp_config_path` removed from `AgentDef` — v2.3 Phase 19 (HOME-01..04)
- ✓ `RC_AGENT_NAME` injected into `.mcp.json` env; memory server warns when absent — v2.3 Phase 19 (HOME-02, HOME-05)
- ✓ Plugin symlink `agent/.claude/plugins → ~/.claude/plugins` for HOME-isolated agents — v2.3 Phase 19
- ✓ `rightclaw init --telegram-token` writes to agent-level `.claude/channels/telegram/` + records `telegram_token_file` in agent.yaml — v2.3 Phase 19
- ✓ Fresh-init UAT: 7 test cases validated end-to-end — v2.3 Phase 19 (HOME-06)
- ✓ `startup_prompt` runs rightcron inline on main thread without Agent tool delegation — v2.5 Phase 21 (BOOT-01, BOOT-02)
- ✓ cronsync SKILL.md CHECK/RECONCILE split with CRITICAL guard against Agent tool delegation — v2.5 Phase 21 (RECON-01, RECON-02)

### Active

- [ ] Per-agent teloxide Telegram bot process (replaces Claude Code channels)
- [ ] Thread → session mapping in memory.db (new `telegram_sessions` table)
- [ ] `claude -p --session-id / --resume` session continuity per Telegram thread
- [ ] System prompt composition from SOUL.md + USER.md + AGENTS.md on `rightclaw up`
- [ ] Cron scheduling/execution in Rust runtime (tokio task, file watcher)
- [ ] Cronsync SKILL.md reduced to file management only (create/edit/delete YAML specs)

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
| Inline bootstrap on main thread (v2.5) | CronCreate is main-thread-only; subagents can't call it | ✓ Good |
| CRITICAL guard + CHECK/RECONCILE split (v2.5) | Structural prevention of Agent tool delegation in reconciler | ✓ Good |

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

**v2.5 shipped** (2026-03-31). RightCron Reliability — fixed startup_prompt Agent tool delegation (bootstrap now runs inline on main thread with CronCreate access), restructured cronsync SKILL.md with CRITICAL guard and CHECK/RECONCILE phase split. Phase 22 (E2E verification) cancelled — user chose new milestone approach. [Full archive](milestones/v2.5-ROADMAP.md)

**Shipped versions:**
- v1.0 (2026-03-23): Core runtime — CLI, process-compose, OpenShell sandbox, Telegram, skills, RightCron
- v2.0 (2026-03-24): Native sandbox — replaced OpenShell with CC sandbox (bubblewrap/Seatbelt)
- v2.1 (2026-03-25): Headless agent isolation — per-agent HOME override + credential symlinks
- v2.2 (2026-03-26): Skills registry — ClawHub removed, `/rightskills` (skills.sh) as built-in
- v2.3 (2026-03-27): Memory system — per-agent SQLite, MCP server, CLI inspection
- v2.4 (2026-03-28): Telegram diagnosis — iv6/M6 gap identified, fix deferred to CC upstream
- v2.5 (2026-03-31): RightCron reliability — inline bootstrap + CHECK/RECONCILE skill redesign

**Known limitations:**
- SEED-002: BOOTSTRAP.md onboarding doesn't trigger via Telegram
- SEED-011: CC channels bug (iv6/M6 gap) — Telegram stops responding after SubagentStop; waiting for CC upstream fix
- VER-01 (rightcron E2E verification) not yet validated — cancelled from v2.5, may be addressed in next milestone
- `rightclaw restart` status unknown — changed `is_tty` to `is_interactive`; restart may now work
- `test_status_no_running_instance` integration test fails (pre-existing)
- Tech debt: git absence warning in `verify_dependencies()` but not surfaced by `rightclaw doctor`

---
*Last updated: 2026-03-31 — Milestone v3.0 started: Teloxide Bot Runtime*
