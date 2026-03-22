# Requirements: RightClaw

**Defined:** 2026-03-21
**Core Value:** Run multiple autonomous Claude Code agents safely -- each sandboxed by OpenShell policies, orchestrated by a single CLI command.

## v1 Requirements

### CLI Lifecycle

- [x] **CLI-01**: User can run `rightclaw up <project-path>` to scan agents/, generate process-compose config, and launch all agents
- [ ] **CLI-02**: User can run `rightclaw up --agents watchdog,reviewer` to launch only specific agents
- [ ] **CLI-03**: User can run `rightclaw up -d` to launch agents in background with TUI server
- [ ] **CLI-04**: User can run `rightclaw attach` to connect to running process-compose TUI
- [x] **CLI-05**: User can run `rightclaw status` to see agent states (running, stopped, restarting)
- [x] **CLI-06**: User can run `rightclaw restart <agent>` to restart a single agent
- [ ] **CLI-07**: User can run `rightclaw down` to stop all agents and destroy sandboxes

### Sandboxing

- [x] **SAND-01**: Each agent launches inside an OpenShell sandbox with its own YAML policy
- [x] **SAND-02**: Shell wrapper per agent reads policy from agent directory and invokes `openshell sandbox create --policy <path> -- claude`
- [x] **SAND-03**: `rightclaw down` explicitly destroys OpenShell sandboxes (signals don't cross container boundaries)
- [ ] **SAND-04**: Shipped default policies use `hard_requirement` for Landlock (no silent degradation on older kernels)
- [ ] **SAND-05**: Shipped default policies cover filesystem, network, and process restrictions — OpenShell validates the YAML, not RightClaw

### Agent Workspace

- [x] **WORK-01**: Agent directory structure follows OpenClaw conventions: SOUL.md, USER.md, IDENTITY.md, MEMORY.md, AGENTS.md, TOOLS.md, BOOTSTRAP.md, HEARTBEAT.md
- [x] **WORK-02**: Each agent can have optional `agent.yaml` for restart policy, backoff seconds, max restarts, and custom start prompt
- [x] **WORK-03**: Each agent can have `.mcp.json` for per-agent MCP server configuration
- [x] **WORK-04**: Each agent must contain a `policy.yaml` file — passed directly to OpenShell, not parsed by RightClaw
- [x] **WORK-05**: Agent directory with IDENTITY.md is auto-detected as a valid agent by `rightclaw up`

### Default Agent

- [ ] **DFLT-01**: RightClaw ships a default "Right" agent in `agents/right/`
- [ ] **DFLT-02**: "Right" agent has BOOTSTRAP.md that runs on first conversation -- asks user's name, vibe, personality, emoji
- [ ] **DFLT-03**: BOOTSTRAP.md onboarding writes IDENTITY.md, USER.md, SOUL.md on completion, then self-deletes
- [ ] **DFLT-04**: "Right" agent is general-purpose -- no domain-specific skills, suitable as a starting template

### Installation

- [ ] **INST-01**: `install.sh` one-liner installs rightclaw binary, process-compose, and OpenShell
- [ ] **INST-02**: `rightclaw doctor` validates all dependencies are present and functional (rightclaw, process-compose, openshell, claude CLI)
- [ ] **INST-03**: `rightclaw doctor` validates agent directory structure and policy files

### Skill Management

- [ ] **SKLM-01**: `/clawhub` Claude Code skill can search ClawHub registry by name or description via HTTP API
- [ ] **SKLM-02**: `/clawhub` skill can install a skill by slug -- downloads to agent's `skills/` directory
- [ ] **SKLM-03**: `/clawhub` skill can uninstall a skill by name -- removes from `skills/` directory
- [ ] **SKLM-04**: `/clawhub` skill can list installed skills for the current agent
- [ ] **SKLM-05**: Policy gate audits skill permissions (from SKILL.md frontmatter `metadata.openclaw.requires`) before activation
- [ ] **SKLM-06**: Skills use standard ClawHub SKILL.md format with YAML frontmatter -- drop-in compatible

### Scheduled Tasks

- [ ] **CRON-01**: CronSync Claude Code skill reads `crons/*.yaml` specs as desired state
- [ ] **CRON-02**: CronSync reconciles desired state against live cron jobs (CronList) via state.json mapping
- [ ] **CRON-03**: CronSync creates missing jobs (CronCreate), deletes orphaned jobs (CronDelete), recreates changed jobs
- [ ] **CRON-04**: Lock-file concurrency control prevents duplicate cron runs -- heartbeat-based with configurable TTL
- [ ] **CRON-05**: All timestamps in lock files use UTC ISO 8601 format (suffix `Z`)
- [ ] **CRON-06**: Cron YAML specs support `schedule` (5-field cron), `lock_ttl`, `max_turns`, and `prompt` fields

### Telegram Channel

- [ ] **CHAN-01**: Per-agent Telegram channel configuration via `.mcp.json` using official Claude Code Telegram plugin
- [ ] **CHAN-02**: Default "Right" agent BOOTSTRAP.md includes Telegram bot setup as part of onboarding
- [ ] **CHAN-03**: OpenShell policy templates include Telegram Bot API endpoint (`api.telegram.org`) in network allowlist

### Project Setup

- [x] **PROJ-01**: Rust project with edition 2024, devenv configuration for Rust toolchain
- [x] **PROJ-02**: CI-ready project structure with tests

## v2 Requirements

### Shared Memory

- **SHMM-01**: MCP memory server (SQLite or knowledge graph) for inter-agent memory sharing
- **SHMM-02**: Tagged memory entries with source agent identification

### Advanced Features

- **ADVN-01**: Policy merging when agent has base policy and installed skills with their own requirements
- **ADVN-02**: Token broker for OAuth token sharing across concurrent Claude Code sessions

## Out of Scope

| Feature | Reason |
|---------|--------|
| Discord, Slack, WhatsApp channels | Telegram only for v1 -- other channels deferred |
| Web UI / dashboard | Anti-feature -- process-compose TUI covers 95% of needs |
| Session management / persistence | Claude Code handles its own sessions |
| Model management / multi-provider | Claude Code-specific, users configure model settings directly |
| Agent-to-agent communication | Phase 2+ if ever -- agents are autonomous |
| Built-in secrets management | Use system-level secrets (env vars, vault) -- agents inherit from shell wrapper |
| Plugin system | Skills (ClawHub-compatible) are the single extensibility mechanism |
| Workflow engine | Claude Code sessions handle task execution |
| Central orchestrator / master agent | Contradicts autonomous agent philosophy |
| `clawhub` CLI as dependency | Skill talks to HTTP API directly |
| Building specific task agents (watchdog, reviewer, scout, ops, forge) | Users define their own agents |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CLI-01 | Phase 2 | Complete |
| CLI-02 | Phase 2 | Pending |
| CLI-03 | Phase 2 | Pending |
| CLI-04 | Phase 2 | Pending |
| CLI-05 | Phase 2 | Complete |
| CLI-06 | Phase 2 | Complete |
| CLI-07 | Phase 2 | Pending |
| SAND-01 | Phase 2 | Complete |
| SAND-02 | Phase 2 | Complete |
| SAND-03 | Phase 2 | Complete |
| SAND-04 | Phase 3 | Pending |
| SAND-05 | Phase 3 | Pending |
| WORK-01 | Phase 1 | Complete |
| WORK-02 | Phase 1 | Complete |
| WORK-03 | Phase 1 | Complete |
| WORK-04 | Phase 1 | Complete |
| WORK-05 | Phase 1 | Complete |
| DFLT-01 | Phase 3 | Pending |
| DFLT-02 | Phase 3 | Pending |
| DFLT-03 | Phase 3 | Pending |
| DFLT-04 | Phase 3 | Pending |
| INST-01 | Phase 3 | Pending |
| INST-02 | Phase 3 | Pending |
| INST-03 | Phase 3 | Pending |
| SKLM-01 | Phase 4 | Pending |
| SKLM-02 | Phase 4 | Pending |
| SKLM-03 | Phase 4 | Pending |
| SKLM-04 | Phase 4 | Pending |
| SKLM-05 | Phase 4 | Pending |
| SKLM-06 | Phase 4 | Pending |
| CRON-01 | Phase 4 | Pending |
| CRON-02 | Phase 4 | Pending |
| CRON-03 | Phase 4 | Pending |
| CRON-04 | Phase 4 | Pending |
| CRON-05 | Phase 4 | Pending |
| CRON-06 | Phase 4 | Pending |
| CHAN-01 | Phase 3 | Pending |
| CHAN-02 | Phase 3 | Pending |
| CHAN-03 | Phase 3 | Pending |
| PROJ-01 | Phase 1 | Complete |
| PROJ-02 | Phase 1 | Complete |

**Coverage:**
- v1 requirements: 41 total
- Mapped to phases: 41
- Unmapped: 0

---
*Requirements defined: 2026-03-21*
*Last updated: 2026-03-21 after roadmap creation*
