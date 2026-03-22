# Roadmap: RightClaw

## Overview

RightClaw delivers a sandboxed multi-agent CLI runtime in four phases. Phase 1 establishes the Rust project and agent workspace model (discovery, config parsing, policy schema). Phase 2 wires up the full CLI lifecycle -- code generation, process-compose integration, and OpenShell sandbox enforcement. Phase 3 ships the default "Right" agent, Telegram channel support, and installation tooling so users get a working first-run experience. Phase 4 adds ecosystem access (ClawHub skills with policy gating) and scheduled automation (CronSync).

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 1: Foundation and Agent Discovery** - Rust project scaffold, core types, agent directory parsing
- [ ] **Phase 2: CLI Runtime and Sandboxing** - Code generation, process-compose lifecycle, OpenShell sandbox enforcement
- [ ] **Phase 3: Default Agent and Installation** - "Right" agent with onboarding, default policies, Telegram channels, install script, doctor command
- [ ] **Phase 4: Skills and Automation** - ClawHub skill management with policy gate, CronSync with lock-file concurrency

## Phase Details

### Phase 1: Foundation and Agent Discovery
**Goal**: Users can define agent workspaces and RightClaw can discover, parse, and validate them
**Depends on**: Nothing (first phase)
**Requirements**: PROJ-01, PROJ-02, WORK-01, WORK-02, WORK-03, WORK-04, WORK-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw --help` prints subcommand listing and the project compiles with edition 2024
  2. Given an `agents/` directory with valid agent subdirectories, RightClaw discovers all agents and parses their `agent.yaml` and `.mcp.json` configs
  3. Agent directories following OpenClaw conventions (IDENTITY.md, SOUL.md, etc.) are recognized as valid agents
  4. Each agent directory requires a `policy.yaml` file — existence is validated, content is passed to OpenShell as-is
**Plans**: 2 plans

Plans:
- [x] 01-01-PLAN.md — Cargo workspace scaffold, devenv, CLI skeleton, core types (AgentDef, AgentConfig, errors, home resolution)
- [x] 01-02-PLAN.md — Agent discovery logic, init command with embedded templates, list command, integration tests

### Phase 2: CLI Runtime and Sandboxing
**Goal**: Users can launch, monitor, and stop sandboxed agents with a single CLI command
**Depends on**: Phase 1
**Requirements**: CLI-01, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, CLI-07, SAND-01, SAND-02, SAND-03
**Success Criteria** (what must be TRUE):
  1. `rightclaw up <path>` generates shell wrappers and process-compose.yaml, then launches all discovered agents inside OpenShell sandboxes
  2. `rightclaw up --agents a,b` launches only the named agents; `rightclaw up -d` launches in background with TUI server
  3. `rightclaw status` shows running/stopped state for each agent; `rightclaw restart <agent>` restarts a single agent
  4. `rightclaw attach` connects to a running process-compose TUI session
  5. `rightclaw down` stops all agents and explicitly destroys OpenShell sandboxes (signals do not cross container boundaries)
**Plans**: 3 plans

Plans:
- [ ] 02-01-PLAN.md — Workspace deps, minijinja templates, codegen module (shell wrapper + process-compose.yaml generation)
- [ ] 02-02-PLAN.md — Runtime module (process-compose REST API client, sandbox tracking/cleanup, dependency verification)
- [ ] 02-03-PLAN.md — Wire CLI subcommands (up, down, status, restart, attach) and integration tests

### Phase 3: Default Agent and Installation
**Goal**: Users can install RightClaw and have a working agent experience out of the box
**Depends on**: Phase 2
**Requirements**: DFLT-01, DFLT-02, DFLT-03, DFLT-04, SAND-04, SAND-05, INST-01, INST-02, INST-03, CHAN-01, CHAN-02, CHAN-03
**Success Criteria** (what must be TRUE):
  1. `install.sh` one-liner installs the rightclaw binary, process-compose, and OpenShell
  2. `rightclaw doctor` validates all dependencies are present and checks agent directory structure and policy files
  3. `rightclaw up` with only the default "Right" agent triggers BOOTSTRAP.md onboarding -- asks name, vibe, personality, writes IDENTITY.md/USER.md/SOUL.md, then self-deletes BOOTSTRAP.md
  4. Default "Right" agent is general-purpose with no domain-specific skills, suitable as a template for new agents
  5. Default policy.yaml files use `hard_requirement` Landlock mode and cover filesystem, network, and process restrictions
  6. Telegram channel configuration works via `.mcp.json` with policy template including `api.telegram.org` in network allowlist
**Plans**: TBD

Plans:
- [ ] 03-01: TBD
- [ ] 03-02: TBD

### Phase 4: Skills and Automation
**Goal**: Agents can safely install ClawHub skills and run scheduled tasks autonomously
**Depends on**: Phase 3
**Requirements**: SKLM-01, SKLM-02, SKLM-03, SKLM-04, SKLM-05, SKLM-06, CRON-01, CRON-02, CRON-03, CRON-04, CRON-05, CRON-06
**Success Criteria** (what must be TRUE):
  1. `/clawhub` Claude Code skill can search, install, uninstall, and list ClawHub skills via HTTP API
  2. Policy gate audits skill permissions from SKILL.md frontmatter against agent sandbox policy before activation -- skills requesting disallowed permissions are blocked
  3. Skills use standard ClawHub SKILL.md format with YAML frontmatter -- drop-in compatible with existing ecosystem
  4. CronSync skill reads `crons/*.yaml` specs and reconciles against live cron jobs -- creates missing, deletes orphaned, recreates changed
  5. Lock-file concurrency control with heartbeat-based TTL prevents duplicate cron runs; all timestamps use UTC ISO 8601 (suffix Z)
**Plans**: TBD

Plans:
- [ ] 04-01: TBD
- [ ] 04-02: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Foundation and Agent Discovery | 0/2 | Not started | - |
| 2. CLI Runtime and Sandboxing | 0/3 | Not started | - |
| 3. Default Agent and Installation | 0/2 | Not started | - |
| 4. Skills and Automation | 0/2 | Not started | - |
