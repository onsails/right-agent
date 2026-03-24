# Roadmap: RightClaw

## Completed Milestones

- **v1.0** (2026-03-21 -> 2026-03-23) -- Multi-agent runtime: Rust CLI, process-compose orchestration, OpenShell sandboxing, Telegram channels, skills.sh integration, RightCron scheduling. [Full roadmap](milestones/v1.0-ROADMAP.md)

<details>
<summary>v2.0 Native Sandbox & Agent Isolation (Shipped: 2026-03-24)</summary>

### Phase 5: Remove OpenShell
**Goal**: Agents launch via direct `claude` invocation instead of OpenShell sandbox wrappers
**Plans:** 2 plans (complete)

### Phase 6: Sandbox Configuration
**Goal**: Each agent launches with CC native sandbox enforced via generated settings.json
**Plans:** 2 plans (complete)

### Phase 7: Platform Compatibility
**Goal**: Users on Linux and macOS get correct dependency guidance and automated installation for the new sandbox stack
**Plans:** 2 plans (complete)

</details>

## Current Milestone: v2.1 Headless Agent Isolation

**Goal:** Make agents fully autonomous without interactive TUI prompts -- complete HOME isolation from host config, explicit permission grants instead of bypass mode, silent sandbox enforcement, pre-populated agent environment.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (8.1, 8.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 8: HOME Isolation & Permission Model** - Per-agent HOME override with credential forwarding, explicit permissions replacing bypass mode
- [ ] **Phase 9: Agent Environment Setup** - Pre-populate complete agent environment: git init, Telegram channel copy, full .claude/ scaffold
- [ ] **Phase 10: Doctor & Managed Settings** - Opt-in managed-settings command and doctor conflict detection

## Phase Details

### Phase 8: HOME Isolation & Permission Model
**Goal**: Each agent runs in its own HOME directory with full CC config isolation, explicit permission grants, and no interactive prompts on launch
**Depends on**: Phase 7 (v2.0)
**Requirements**: HOME-01, HOME-02, HOME-03, HOME-04, HOME-05, PERM-01, PERM-02
**Success Criteria** (what must be TRUE):
  1. Each agent's shell wrapper sets `HOME` to the agent directory -- agent sees only its own `.claude/`, `.claude.json`, sessions, and memory
  2. `rightclaw up` generates a per-agent `.claude.json` with workspace trust entries and bypass-accepted state inside the agent directory
  3. Host OAuth credentials are symlinked into each agent's `.claude/.credentials.json` so agents authenticate without copies going stale
  4. Shell wrapper forwards git identity (`GIT_CONFIG_GLOBAL`), SSH (`SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`), and author info so git operations work under HOME override
  5. All generated sandbox `allowWrite`/`denyRead` paths use absolute paths (not `~/` which resolves to agent HOME, not real HOME)
**Plans:** 0/2 plans executed
Plans:
- [x] 08-01-PLAN.md -- Shell wrapper HOME override, env forwarding, .claude.json generation, credential symlink
- [x] 08-02-PLAN.md -- Sandbox absolute path hardening, allowRead support, integration tests

### Phase 9: Agent Environment Setup
**Goal**: Agents launch with a fully pre-populated environment -- no CC prompts for missing config, trust dialogs, or channel setup
**Depends on**: Phase 8
**Requirements**: AENV-01, AENV-02, AENV-03, PERM-03
**Success Criteria** (what must be TRUE):
  1. Each agent directory contains a `.git/` directory so CC recognizes it as a trusted workspace without prompting
  2. When Telegram is configured, `rightclaw up` copies channel config to the agent HOME's `.claude/channels/telegram/` so the Telegram plugin finds it under HOME override
  3. Each agent's `.claude/` contains pre-created `settings.json`, `settings.local.json` (empty `{}`), and `skills/` directory -- CC never triggers protected directory write prompts
  4. Telegram channel remains functional as a permission relay safety net for any edge case prompts that bypass suppression
**Plans:** 1/2 plans executed
Plans:
- [x] 09-01-PLAN.md -- AgentConfig Telegram fields, codegen/telegram.rs, codegen/skills.rs, init.rs refactor
- [x] 09-02-PLAN.md -- Wire cmd_up steps 6-9, git Warn doctor check

### Phase 10: Doctor & Managed Settings
**Goal**: Users can opt into machine-wide domain blocking and get warned about managed settings conflicts
**Depends on**: Phase 8 (no dependency on Phase 9)
**Requirements**: TOOL-01, TOOL-02
**Success Criteria** (what must be TRUE):
  1. `rightclaw config strict-sandbox` writes `/etc/claude-code/managed-settings.json` with `allowManagedDomainsOnly: true` (requires sudo, prompts user)
  2. `rightclaw doctor` detects existing `/etc/claude-code/managed-settings.json` and warns if it may conflict with RightClaw's per-agent settings
**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 8 -> 9 -> 10

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 5. Remove OpenShell | v2.0 | 2/2 | Complete | 2026-03-24 |
| 6. Sandbox Configuration | v2.0 | 2/2 | Complete | 2026-03-24 |
| 7. Platform Compatibility | v2.0 | 2/2 | Complete | 2026-03-24 |
| 8. HOME Isolation & Permission Model | v2.1 | 0/2 | Planned    |  |
| 9. Agent Environment Setup | v2.1 | 1/2 | In Progress|  |
| 10. Doctor & Managed Settings | v2.1 | 0/? | Not started | - |
